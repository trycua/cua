package com.cuatest.touchtest;

import android.view.MotionEvent;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

/**
 * Lightweight, serialisable representation of a single MotionEvent sample.
 *
 * <p>Written to Logcat with tag {@code TouchTest} so that the test harness can
 * read results via {@code adb logcat -s TouchTest -d} without requiring any
 * file-system permissions.
 */
public class TouchEvent {

    public final String action;
    public final int pointerCount;
    public final int[] ids;
    public final float[] xs;
    public final float[] ys;
    public final float[] pressures;
    public final long eventTimeMs;
    public final long sequenceId;   // monotonically increasing across the session

    private static long sSequence = 0;

    private TouchEvent(String action, int pointerCount,
                       int[] ids, float[] xs, float[] ys, float[] pressures,
                       long eventTimeMs) {
        this.action = action;
        this.pointerCount = pointerCount;
        this.ids = ids;
        this.xs = xs;
        this.ys = ys;
        this.pressures = pressures;
        this.eventTimeMs = eventTimeMs;
        this.sequenceId = ++sSequence;
    }

    /** Reset the per-session sequence counter (called when the app receives RESET_LOG). */
    public static void resetSequence() {
        sSequence = 0;
    }

    /**
     * Converts a raw {@link MotionEvent} into a {@link TouchEvent}.
     *
     * <p>All active pointers are captured, regardless of which pointer triggered
     * the action.
     */
    public static TouchEvent from(MotionEvent ev) {
        int maskedAction = ev.getActionMasked();
        String actionName;
        switch (maskedAction) {
            case MotionEvent.ACTION_DOWN:           actionName = "ACTION_DOWN";          break;
            case MotionEvent.ACTION_UP:             actionName = "ACTION_UP";            break;
            case MotionEvent.ACTION_MOVE:           actionName = "ACTION_MOVE";          break;
            case MotionEvent.ACTION_CANCEL:         actionName = "ACTION_CANCEL";        break;
            case MotionEvent.ACTION_POINTER_DOWN:   actionName = "ACTION_POINTER_DOWN";  break;
            case MotionEvent.ACTION_POINTER_UP:     actionName = "ACTION_POINTER_UP";    break;
            case MotionEvent.ACTION_OUTSIDE:        actionName = "ACTION_OUTSIDE";       break;
            default:                               actionName = "ACTION_" + maskedAction; break;
        }

        int count = ev.getPointerCount();
        int[] ids        = new int[count];
        float[] xs       = new float[count];
        float[] ys       = new float[count];
        float[] pressures = new float[count];

        for (int i = 0; i < count; i++) {
            ids[i]        = ev.getPointerId(i);
            xs[i]         = ev.getX(i);
            ys[i]         = ev.getY(i);
            pressures[i]  = ev.getPressure(i);
        }

        return new TouchEvent(actionName, count, ids, xs, ys, pressures, ev.getEventTime());
    }

    /** Serialise to a compact JSON string (one line, no pretty-printing). */
    public String toJson() {
        try {
            JSONObject obj = new JSONObject();
            obj.put("seq", sequenceId);
            obj.put("action", action);
            obj.put("pointer_count", pointerCount);
            obj.put("event_time_ms", eventTimeMs);

            JSONArray pointers = new JSONArray();
            for (int i = 0; i < pointerCount; i++) {
                JSONObject p = new JSONObject();
                p.put("id", ids[i]);
                p.put("x", Math.round(xs[i]));
                p.put("y", Math.round(ys[i]));
                p.put("pressure", (float) Math.round(pressures[i] * 100) / 100f);
                pointers.put(p);
            }
            obj.put("pointers", pointers);
            return obj.toString();
        } catch (JSONException e) {
            return "{\"error\":\"json_serialisation_failed\"}";
        }
    }
}
