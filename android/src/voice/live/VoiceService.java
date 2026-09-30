package voice.live;

import android.Manifest;
import android.app.ActivityManager;
import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.Service;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.content.pm.ServiceInfo;
import android.net.Uri;
import android.os.Binder;
import android.os.Build;
import android.os.IBinder;
import android.os.PowerManager;
import android.view.ViewGroup;
import android.webkit.PermissionRequest;
import android.webkit.RenderProcessGoneDetail;
import android.webkit.WebChromeClient;
import android.webkit.WebResourceRequest;
import android.webkit.WebView;
import android.webkit.WebViewClient;

public final class VoiceService extends Service {
    static final String PAGE = "https://bddap-bot.github.io/voice/";
    WebView web;
    private PowerManager.WakeLock wake;
    public final class LocalBinder extends Binder {
        VoiceService service() { return VoiceService.this; }
    }
    public IBinder onBind(Intent intent) { return new LocalBinder(); }
    static boolean allowed(Uri uri) {
        return "https".equals(uri.getScheme()) && "bddap-bot.github.io".equals(uri.getHost())
            && (uri.getPort() == -1 || uri.getPort() == 443)
            && ("/voice/".equals(uri.getPath()) || "/voice/index.html".equals(uri.getPath()));
    }
    public int onStartCommand(Intent intent, int flags, int id) {
        if (intent != null && "stop".equals(intent.getAction())) {
            close();
            return START_NOT_STICKY;
        }
        if (intent != null && "mute".equals(intent.getAction())) {
            if (web != null && allowed(Uri.parse(web.getUrl() == null ? "" : web.getUrl())))
                web.evaluateJavascript("document.getElementById('mic-mute')?.click()", null);
            else if (web == null) stopSelf();
            return START_NOT_STICKY;
        }
        if (web != null) return START_NOT_STICKY;
        getSystemService(NotificationManager.class).createNotificationChannel(
            new NotificationChannel("conversation", "Voice conversation", NotificationManager.IMPORTANCE_LOW));
        PendingIntent open = PendingIntent.getActivity(this, 0, new Intent(this, MainActivity.class), PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
        PendingIntent stop = PendingIntent.getService(this, 1, new Intent(this, VoiceService.class).setAction("stop"), PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
        PendingIntent mute = PendingIntent.getService(this, 2, new Intent(this, VoiceService.class).setAction("mute"), PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
        Notification notice = new Notification.Builder(this, "conversation")
            .setSmallIcon(android.R.drawable.ic_btn_speak_now).setContentTitle("Live Voice is open")
            .setContentText("Mute toggles the page microphone · Close ends the session")
            .setContentIntent(open).setOngoing(true)
            .addAction(new Notification.Action.Builder(null, "Mute", mute).build())
            .addAction(new Notification.Action.Builder(null, "Close", stop).build()).build();
        if (Build.VERSION.SDK_INT >= 30) startForeground(1, notice, ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE | ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK);
        else startForeground(1, notice);
        wake = getSystemService(PowerManager.class).newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "LiveVoice:conversation");
        wake.acquire();
        WebView.setWebContentsDebuggingEnabled((getApplicationInfo().flags & android.content.pm.ApplicationInfo.FLAG_DEBUGGABLE) != 0);
        web = new WebView(this);
        web.getSettings().setJavaScriptEnabled(true);
        web.getSettings().setDomStorageEnabled(true);
        web.getSettings().setMediaPlaybackRequiresUserGesture(false);
        web.getSettings().setAllowFileAccess(false);
        web.getSettings().setAllowContentAccess(false);
        web.setRendererPriorityPolicy(WebView.RENDERER_PRIORITY_IMPORTANT, false);
        web.setWebViewClient(new WebViewClient() {
            public boolean shouldOverrideUrlLoading(WebView view, WebResourceRequest request) {
                return !request.isForMainFrame() || !allowed(request.getUrl());
            }
            public void onPageStarted(WebView view, String url, android.graphics.Bitmap icon) {
                if (!allowed(Uri.parse(url))) { view.stopLoading(); close(); }
            }
            public boolean onRenderProcessGone(WebView view, RenderProcessGoneDetail detail) {
                close();
                return true;
            }
        });
        web.setWebChromeClient(new WebChromeClient() {
            public void onPermissionRequest(PermissionRequest request) {
                Uri origin = request.getOrigin();
                if (web == null || !allowed(Uri.parse(web.getUrl() == null ? "" : web.getUrl()))
                    || !"https".equals(origin.getScheme()) || !"bddap-bot.github.io".equals(origin.getHost())
                    || (origin.getPort() != -1 && origin.getPort() != 443)
                    || checkSelfPermission(Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
                    request.deny();
                    return;
                }
                for (String resource : request.getResources()) if (PermissionRequest.RESOURCE_AUDIO_CAPTURE.equals(resource)) {
                    request.grant(new String[]{PermissionRequest.RESOURCE_AUDIO_CAPTURE});
                    return;
                }
                request.deny();
            }
        });
        web.loadUrl(PAGE);
        return START_NOT_STICKY;
    }
    public void onTaskRemoved(Intent rootIntent) { close(); }
    private void close() {
        if (web != null) {
            ViewGroup parent = (ViewGroup) web.getParent();
            if (parent != null) parent.removeView(web);
            web.destroy();
            web = null;
        }
        if (wake != null && wake.isHeld()) wake.release();
        stopForeground(STOP_FOREGROUND_REMOVE);
        stopSelf();
        for (ActivityManager.AppTask task : getSystemService(ActivityManager.class).getAppTasks()) task.finishAndRemoveTask();
    }
    public void onDestroy() {
        close();
        super.onDestroy();
    }
}
