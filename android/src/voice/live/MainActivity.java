package voice.live;

import android.Manifest;
import android.app.Activity;
import android.content.ComponentName;
import android.content.Intent;
import android.content.ServiceConnection;
import android.content.pm.PackageManager;
import android.os.Build;
import android.os.Bundle;
import android.os.IBinder;
import android.view.ViewGroup;
import android.widget.Button;
import android.widget.LinearLayout;
import android.widget.TextView;

public final class MainActivity extends Activity {
    private VoiceService service;
    private boolean bound;
    private LinearLayout content;
    private final ServiceConnection connection = new ServiceConnection() {
        public void onServiceConnected(ComponentName name, IBinder binder) {
            service = ((VoiceService.LocalBinder) binder).service();
            service.onClosed = () -> finish();
            if (service.web == null) { finish(); return; }
            ViewGroup parent = (ViewGroup) service.web.getParent();
            if (parent != null) parent.removeView(service.web);
            content.removeAllViews();
            Button stop = new Button(MainActivity.this);
            stop.setText("Stop and close");
            stop.setOnClickListener(view -> {
                startService(new Intent(MainActivity.this, VoiceService.class).setAction("stop"));
                finish();
            });
            content.addView(stop);
            Button status = new Button(MainActivity.this);
            status.setText("Audio status");
            status.setOnClickListener(view -> service.web.evaluateJavascript(
                "localStorage.getItem('voice.audio-diagnostics.v1') || '[]'", value -> {
                    String report;
                    try { report = new org.json.JSONArray("[" + value + "]").getString(0); }
                    catch (org.json.JSONException error) { report = "Audio status unavailable"; }
                    final String text = report;
                    new android.app.AlertDialog.Builder(MainActivity.this).setTitle("Audio status")
                        .setMessage(text).setPositiveButton("Close", null)
                        .setNeutralButton("Copy", (dialog, which) -> {
                            android.content.ClipboardManager clipboard = getSystemService(android.content.ClipboardManager.class);
                            clipboard.setPrimaryClip(android.content.ClipData.newPlainText("Audio status", text));
                        }).show();
                }));
            content.addView(status);
            content.addView(service.web, new LinearLayout.LayoutParams(-1, 0, 1));
        }
        public void onServiceDisconnected(ComponentName name) { service = null; finish(); }
    };
    public void onCreate(Bundle state) {
        super.onCreate(state);
        content = new LinearLayout(this);
        content.setOrientation(LinearLayout.VERTICAL);
        content.setFitsSystemWindows(true);
        setContentView(content);
        TextView description = new TextView(this);
        description.setText("Live Voice continues listening while you use other apps or lock the screen. Stop and close ends the session. Your connection token is saved on this device.");
        content.addView(description);
        Button start = new Button(this);
        start.setText("Open Live Voice");
        start.setOnClickListener(view -> {
            if (checkSelfPermission(Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
                requestPermissions(Build.VERSION.SDK_INT >= 33
                    ? new String[]{Manifest.permission.RECORD_AUDIO, Manifest.permission.POST_NOTIFICATIONS}
                    : new String[]{Manifest.permission.RECORD_AUDIO}, 1);
            } else open();
        });
        content.addView(start);
        if (VoiceService.running) open();
    }
    public void onRequestPermissionsResult(int code, String[] permissions, int[] results) {
        super.onRequestPermissionsResult(code, permissions, results);
        if (code == 1 && checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) open();
    }
    private void open() {
        if (bound) return;
        Intent intent = new Intent(this, VoiceService.class);
        startForegroundService(intent);
        bound = bindService(intent, connection, BIND_AUTO_CREATE);
    }
    protected void onDestroy() {
        if (service != null && service.web != null && service.web.getParent() == content) content.removeView(service.web);
        if (service != null) service.onClosed = null;
        if (bound) unbindService(connection);
        super.onDestroy();
    }
}
