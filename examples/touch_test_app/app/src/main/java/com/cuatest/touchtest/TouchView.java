package com.cuatest.touchtest;

import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.Paint;
import android.util.Log;
import android.view.MotionEvent;
import android.view.View;

import java.util.ArrayList;
import java.util.List;

/**
 * Full-screen view that:
 * <ul>
 *   <li>Intercepts every {@link MotionEvent} (including multi-touch).</li>
 *   <li>Logs each event as a JSON line to Logcat under tag {@code TouchTest}.</li>
 *   <li>Draws live touch indicators on a black canvas for visual debugging.</li>
 * </ul>
 *
 * <h3>Reading results</h3>
 * <pre>
 *   adb logcat -s TouchTest -d
 * </pre>
 * Each line has the form:
 * <pre>
 *   D TouchTest: {"seq":1,"action":"ACTION_DOWN","pointer_count":1,...}
 * </pre>
 *
 * <h3>Resetting the log</h3>
 * Clear Logcat before each test so results are unambiguous:
 * <pre>
 *   adb logcat -c
 * </pre>
 */
public class TouchView extends View {

    public static final String LOG_TAG = "TouchTest";

    // Marker emitted once the view is ready; the test harness waits for this.
    public static final String READY_MARKER = "{\"type\":\"READY\",\"msg\":\"TouchView is accepting events\"}";
    // Marker emitted after a RESET_LOG broadcast is handled.
    public static final String RESET_MARKER = "{\"type\":\"RESET\",\"msg\":\"event log cleared\"}";

    private final Paint mCirclePaint = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint mLabelPaint  = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint mInfoPaint   = new Paint(Paint.ANTI_ALIAS_FLAG);

    // Snapshot of active pointers for rendering
    private final List<float[]> mActivePointers = new ArrayList<>();
    private String mLastAction = "—";
    private int mLastPointerCount = 0;

    public TouchView(Context context) {
        super(context);

        mCirclePaint.setColor(Color.parseColor("#4FC3F7"));
        mCirclePaint.setStyle(Paint.Style.FILL);
        mCirclePaint.setAlpha(200);

        mLabelPaint.setColor(Color.WHITE);
        mLabelPaint.setTextSize(32f);
        mLabelPaint.setTextAlign(Paint.Align.CENTER);

        mInfoPaint.setColor(Color.parseColor("#B0BEC5"));
        mInfoPaint.setTextSize(40f);

        // Signal that the view is ready
        Log.d(LOG_TAG, READY_MARKER);
    }

    /** Called by MainActivity when it receives the RESET_LOG broadcast. */
    public void resetLog() {
        TouchEvent.resetSequence();
        mActivePointers.clear();
        mLastAction = "—";
        mLastPointerCount = 0;
        invalidate();
        Log.d(LOG_TAG, RESET_MARKER);
    }

    @Override
    public boolean onTouchEvent(MotionEvent ev) {
        // Capture and log the event
        TouchEvent te = TouchEvent.from(ev);
        Log.d(LOG_TAG, te.toJson());

        // Update render state
        mLastAction = te.action;
        mLastPointerCount = te.pointerCount;
        mActivePointers.clear();
        if (ev.getActionMasked() != MotionEvent.ACTION_UP &&
                ev.getActionMasked() != MotionEvent.ACTION_CANCEL) {
            for (int i = 0; i < ev.getPointerCount(); i++) {
                mActivePointers.add(new float[]{ev.getX(i), ev.getY(i)});
            }
        }
        invalidate();
        return true;  // consume all events
    }

    @Override
    protected void onDraw(Canvas canvas) {
        canvas.drawColor(Color.BLACK);

        // Draw touch circles
        for (int i = 0; i < mActivePointers.size(); i++) {
            float[] p = mActivePointers.get(i);
            canvas.drawCircle(p[0], p[1], 80f, mCirclePaint);
            canvas.drawText(String.valueOf(i), p[0], p[1] + 12f, mLabelPaint);
        }

        // HUD
        float margin = 60f;
        canvas.drawText("Action : " + mLastAction,          margin, 120f,  mInfoPaint);
        canvas.drawText("Pointers: " + mLastPointerCount,   margin, 180f,  mInfoPaint);
        canvas.drawText("Active  : " + mActivePointers.size(), margin, 240f, mInfoPaint);

        // Centre prompt when idle
        if (mActivePointers.isEmpty()) {
            Paint p = new Paint(mInfoPaint);
            p.setTextAlign(Paint.Align.CENTER);
            p.setColor(Color.parseColor("#546E7A"));
            canvas.drawText("Touch here to test", getWidth() / 2f, getHeight() / 2f, p);
        }
    }
}
