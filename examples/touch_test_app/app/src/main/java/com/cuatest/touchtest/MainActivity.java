package com.cuatest.touchtest;

import android.app.Activity;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.os.Build;
import android.os.Bundle;
import android.util.Log;
import android.view.Window;
import android.view.WindowManager;

/**
 * Single-activity host for {@link TouchView}.
 *
 * <p>Registers a {@link BroadcastReceiver} for the action
 * {@code com.cuatest.touchtest.RESET_LOG} so the test harness can atomically
 * clear the in-memory sequence counter between test cases:
 * <pre>
 *   adb shell am broadcast -a com.cuatest.touchtest.RESET_LOG
 * </pre>
 */
public class MainActivity extends Activity {

    private static final String TAG = "TouchTest";
    private TouchView mTouchView;

    private final BroadcastReceiver mResetReceiver = new BroadcastReceiver() {
        @Override
        public void onReceive(Context ctx, Intent intent) {
            if ("com.cuatest.touchtest.RESET_LOG".equals(intent.getAction())) {
                Log.d(TAG, "{\"type\":\"BROADCAST\",\"msg\":\"RESET_LOG received\"}");
                if (mTouchView != null) {
                    mTouchView.resetLog();
                }
            }
        }
    };

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);

        // Fullscreen, keep screen on
        requestWindowFeature(Window.FEATURE_NO_TITLE);
        getWindow().setFlags(
                WindowManager.LayoutParams.FLAG_FULLSCREEN |
                WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON,
                WindowManager.LayoutParams.FLAG_FULLSCREEN |
                WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON
        );

        mTouchView = new TouchView(this);
        setContentView(mTouchView);

        // Log device capabilities
        Log.d(TAG, "{\"type\":\"DEVICE_INFO\","
                + "\"model\":\"" + Build.MODEL + "\","
                + "\"sdk\":" + Build.VERSION.SDK_INT + ","
                + "\"multitouch_distinct\":"
                + getPackageManager().hasSystemFeature("android.hardware.touchscreen.multitouch.distinct")
                + "}");
    }

    @Override
    protected void onResume() {
        super.onResume();
        IntentFilter filter = new IntentFilter("com.cuatest.touchtest.RESET_LOG");
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            registerReceiver(mResetReceiver, filter, Context.RECEIVER_NOT_EXPORTED);
        } else {
            registerReceiver(mResetReceiver, filter);
        }
        Log.d(TAG, "{\"type\":\"LIFECYCLE\",\"msg\":\"onResume — app is active\"}");
    }

    @Override
    protected void onPause() {
        super.onPause();
        try { unregisterReceiver(mResetReceiver); } catch (IllegalArgumentException ignored) {}
        Log.d(TAG, "{\"type\":\"LIFECYCLE\",\"msg\":\"onPause\"}");
    }
}
