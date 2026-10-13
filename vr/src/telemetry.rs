use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use opentelemetry::trace::TracerProvider;
use opentelemetry::{Context, KeyValue};
use opentelemetry_otlp::{LogExporter, SpanExporter, WithExportConfig};
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::trace::{BatchSpanProcessor, SdkTracerProvider, Span, SpanData, SpanProcessor};
use opentelemetry_sdk::Resource;
use tracing::Level;
use tracing_subscriber::filter::{filter_fn, Targets};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::Layer;

const SERVICE: &str = "voice-vr";
/// Two refreshes at 90 Hz, so a frame span this long is a dropped frame.
const SLOW_FRAME: Duration = Duration::from_millis(20);
const FRAME_SAMPLE: Duration = Duration::from_secs(1);

static LOGS: OnceLock<SdkLoggerProvider> = OnceLock::new();

#[must_use = "unexported telemetry is lost when the guard drops"]
pub struct Guard {
    logs: Option<SdkLoggerProvider>,
    traces: Option<SdkTracerProvider>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(traces) = &self.traces {
            let _ = traces.shutdown();
        }
        if let Some(logs) = &self.logs {
            let _ = logs.shutdown();
        }
    }
}

/// Logs go to stderr, and with `OTEL_EXPORTER_OTLP_ENDPOINT` set also to that OTLP/HTTP endpoint with the spans; a panic leaves a report in `crashes` until it is delivered, and the reports left there are delivered in the background.
pub fn init(crashes: PathBuf) -> Guard {
    let level = Targets::new().with_default(Level::WARN).with_target("voice_vr", Level::INFO);
    let stderr = tracing_subscriber::fmt::layer().with_writer(std::io::stderr).without_time().with_target(false).with_ansi(false);
    let endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok().filter(|endpoint| !endpoint.is_empty());
    let providers = endpoint.as_ref().map(|_| providers());
    let (logs, traces, failure) = match providers {
        Some(Ok((logs, traces))) => (Some(logs), Some(traces), None),
        Some(Err(error)) => (None, None, Some(error)),
        None => (None, None, None),
    };
    let log_layer = logs.as_ref().map(|logs| opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new(logs).with_filter(filter_fn(|meta| exportable(meta.target()))));
    let span_layer = traces.as_ref().map(|traces| tracing_opentelemetry::layer().with_tracer(traces.tracer(SERVICE)).with_filter(filter_fn(|meta| exportable(meta.target()))));
    tracing_subscriber::registry().with(level).with(stderr).with(log_layer).with(span_layer).init();
    match (&endpoint, failure) {
        (Some(endpoint), None) => tracing::info!(endpoint, "OTLP telemetry on"),
        (_, Some(error)) => tracing::warn!("OTLP telemetry off: {error}"),
        (None, None) => {}
    }
    if let Some(logs) = &logs {
        let _ = LOGS.set(logs.clone());
    }
    let crashes_hook = crashes.clone();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let report = format!("panic in thread {}: {info}\n{}", thread.name().unwrap_or("unnamed"), std::backtrace::Backtrace::force_capture());
        let written = write_report(&crashes_hook, &report);
        tracing::error!(target: "voice_vr::crash", "{report}");
        if thread.name().is_some_and(|name| name.starts_with("OpenTelemetry")) {
            return;
        }
        if let (Ok(path), true) = (written, delivered()) {
            let _ = std::fs::remove_file(path);
        }
    }));
    std::thread::spawn(move || ship(&crashes));
    Guard { logs, traces }
}

/// The export pipeline's own crates log through the subscriber it feeds, so exporting their records would loop.
fn exportable(target: &str) -> bool {
    !["opentelemetry", "hyper", "reqwest", "h2", "tonic", "tower"].iter().any(|internal| target.starts_with(internal))
}

fn providers() -> Result<(SdkLoggerProvider, SdkTracerProvider), String> {
    // iroh's TLS feature leaves reqwest with no crypto provider of its own, and the exporters' client refuses to build without one.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut resource = Resource::builder().with_service_name(SERVICE);
    if !std::env::var("OTEL_RESOURCE_ATTRIBUTES").is_ok_and(|attributes| attributes.contains("host.name=")) {
        resource = resource.with_attribute(KeyValue::new("host.name", hostname()));
    }
    let resource = resource.with_attribute(KeyValue::new("service.version", env!("CARGO_PKG_VERSION"))).build();
    let log_exporter = LogExporter::builder().with_http().with_timeout(Duration::from_secs(5)).build().map_err(|error| error.to_string())?;
    let span_exporter = SpanExporter::builder().with_http().with_timeout(Duration::from_secs(5)).build().map_err(|error| error.to_string())?;
    let logs = SdkLoggerProvider::builder().with_resource(resource.clone()).with_batch_exporter(log_exporter).build();
    let frames = Frames { inner: BatchSpanProcessor::builder(span_exporter).build(), last: Mutex::new(UNIX_EPOCH) };
    let traces = SdkTracerProvider::builder().with_resource(resource).with_span_processor(frames).build();
    Ok((logs, traces))
}

fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname").map(|name| name.trim().to_owned()).unwrap_or_else(|_| "unknown".to_owned())
}

fn delivered() -> bool {
    LOGS.get().is_some_and(|logs| logs.force_flush().is_ok())
}

fn write_report(crashes: &Path, report: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(crashes)?;
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis();
    let path = crashes.join(format!("{stamp}-{}.txt", std::process::id()));
    std::fs::write(&path, report)?;
    Ok(path)
}

fn ship(crashes: &Path) {
    let Ok(entries) = std::fs::read_dir(crashes) else { return };
    let mut reports: Vec<PathBuf> = entries.filter_map(|entry| Some(entry.ok()?.path())).collect();
    reports.sort();
    if reports.is_empty() {
        return;
    }
    for report in &reports {
        let text = std::fs::read_to_string(report).unwrap_or_else(|error| format!("unreadable: {error}"));
        tracing::error!(target: "voice_vr::crash", "an earlier run crashed ({}): {text}", report.file_name().unwrap_or_default().to_string_lossy());
    }
    if delivered() {
        for report in &reports {
            let _ = std::fs::remove_file(report);
        }
    }
}

/// Keeps every slow frame span and one other a second; a span for every frame would ship the overlay's refresh rate in spans.
#[derive(Debug)]
struct Frames<P> {
    inner: P,
    last: Mutex<SystemTime>,
}

impl<P: SpanProcessor> SpanProcessor for Frames<P> {
    fn on_start(&self, span: &mut Span, cx: &Context) {
        self.inner.on_start(span, cx);
    }

    fn on_end(&self, span: SpanData) {
        if span.name == "frame" {
            let took = span.end_time.duration_since(span.start_time).unwrap_or_default();
            let mut last = self.last.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if took < SLOW_FRAME && span.end_time.duration_since(*last).map_or(true, |since| since < FRAME_SAMPLE) {
                return;
            }
            *last = span.end_time;
        }
        self.inner.on_end(span);
    }

    fn force_flush(&self) -> OTelSdkResult {
        self.inner.force_flush()
    }

    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        self.inner.shutdown_with_timeout(timeout)
    }

    fn set_resource(&mut self, resource: &Resource) {
        self.inner.set_resource(resource);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exports_all_but_the_pipeline() {
        for target in ["opentelemetry_sdk::logs", "hyper_util::client", "reqwest::blocking", "h2::codec"] {
            assert!(!exportable(target), "{target}");
        }
        for target in ["voice_vr", "voice_vr::crash", "iroh::endpoint"] {
            assert!(exportable(target), "{target}");
        }
    }

    #[derive(Debug, Default)]
    struct Kept(Mutex<Vec<String>>);

    impl SpanProcessor for &Kept {
        fn on_start(&self, _: &mut Span, _: &Context) {}
        fn on_end(&self, span: SpanData) {
            self.0.lock().unwrap().push(format!("{} {}", span.name, span.start_time.duration_since(UNIX_EPOCH).unwrap().as_millis()));
        }
        fn force_flush(&self) -> OTelSdkResult {
            Ok(())
        }
        fn shutdown_with_timeout(&self, _: Duration) -> OTelSdkResult {
            Ok(())
        }
    }

    fn span(name: &'static str, start_ms: u64, length_ms: u64) -> SpanData {
        let start = UNIX_EPOCH + Duration::from_millis(start_ms);
        SpanData {
            span_context: opentelemetry::trace::SpanContext::empty_context(),
            parent_span_id: opentelemetry::trace::SpanId::INVALID,
            parent_span_is_remote: false,
            span_kind: opentelemetry::trace::SpanKind::Internal,
            name: name.into(),
            start_time: start,
            end_time: start + Duration::from_millis(length_ms),
            attributes: Vec::new(),
            dropped_attributes_count: 0,
            events: Default::default(),
            links: Default::default(),
            status: opentelemetry::trace::Status::Unset,
            instrumentation_scope: Default::default(),
        }
    }

    #[test]
    fn keeps_slow_frames_one_more_a_second_and_every_other_span() {
        let kept = Kept::default();
        let frames = Frames { inner: &kept, last: Mutex::new(UNIX_EPOCH) };
        let mut start = 100_000;
        for frame in 0..270 {
            let length = if frame == 100 { 30 } else { 11 };
            frames.on_end(span("frame", start, length));
            start += length;
        }
        frames.on_end(span("frame", 99_000, 11));
        frames.on_end(span("session", 100_000, 3_000));
        assert_eq!(*kept.0.lock().unwrap(), ["frame 100000", "frame 101001", "frame 101100", "frame 102120", "session 100000"]);
    }
}
