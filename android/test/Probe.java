package voice.live;

import android.app.Instrumentation;
import android.content.Intent;
import android.os.Bundle;
import android.webkit.WebView;

public final class Probe extends Instrumentation {
    public void onCreate(Bundle args) {
        super.onCreate(args);
        start();
    }
    private void clickOpen(android.view.View view) {
        if (view instanceof android.widget.Button && "Open Live Voice".contentEquals(((android.widget.Button) view).getText())) view.performClick();
        else if (view instanceof android.view.ViewGroup) {
            android.view.ViewGroup group = (android.view.ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) clickOpen(group.getChildAt(i));
        }
    }
    public void onStart() {
        runOnMainSync(() -> WebView.setWebContentsDebuggingEnabled(true));
        android.app.Activity activity = startActivitySync(new Intent(getTargetContext(), MainActivity.class).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));
        runOnMainSync(() -> clickOpen(activity.getWindow().getDecorView()));
        try { Thread.sleep(600000); }
        catch (InterruptedException error) { Thread.currentThread().interrupt(); }
        finish(0, new Bundle());
    }
}
