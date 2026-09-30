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
import android.widget.FrameLayout;

public final class MainActivity extends Activity {
    private VoiceService service;
    private boolean bound;
    private FrameLayout content;
    private final ServiceConnection connection = new ServiceConnection() {
        public void onServiceConnected(ComponentName name, IBinder binder) {
            service = ((VoiceService.LocalBinder) binder).service();
            if (service.web == null) { finish(); return; }
            ViewGroup parent = (ViewGroup) service.web.getParent();
            if (parent != null) parent.removeView(service.web);
            content.removeAllViews();
            content.addView(service.web, new FrameLayout.LayoutParams(-1, -1));
        }
        public void onServiceDisconnected(ComponentName name) { service = null; finish(); }
    };
    public void onCreate(Bundle state) {
        super.onCreate(state);
        content = new FrameLayout(this);
        content.setFitsSystemWindows(true);
        setContentView(content);
        if (checkSelfPermission(Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
            requestPermissions(Build.VERSION.SDK_INT >= 33
                ? new String[]{Manifest.permission.RECORD_AUDIO, Manifest.permission.POST_NOTIFICATIONS}
                : new String[]{Manifest.permission.RECORD_AUDIO}, 1);
        } else open();
    }
    public void onRequestPermissionsResult(int code, String[] permissions, int[] results) {
        super.onRequestPermissionsResult(code, permissions, results);
        if (code == 1 && checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) open();
        else finishAndRemoveTask();
    }
    private void open() {
        if (bound) return;
        Intent intent = new Intent(this, VoiceService.class);
        startForegroundService(intent);
        bound = bindService(intent, connection, BIND_AUTO_CREATE);
    }
    protected void onDestroy() {
        if (service != null && service.web != null && service.web.getParent() == content) content.removeView(service.web);
        if (bound) unbindService(connection);
        super.onDestroy();
    }
}
