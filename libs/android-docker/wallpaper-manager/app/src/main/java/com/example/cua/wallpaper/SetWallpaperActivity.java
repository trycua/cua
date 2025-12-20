package com.example.cua.wallpaper;

import android.app.Activity;
import android.app.WallpaperManager;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.os.Build;
import android.os.Bundle;
import android.util.Log;

import java.io.File;

public class SetWallpaperActivity extends Activity {

    private static final String TAG = "CuaWallpaperManager";

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);

        // Expected extras:
        //   "path"   -> absolute path to image on device (e.g. /sdcard/Pictures/wall.jpg)
        //   "target" -> optional: "home", "lock", or "both" (default: "home")
        String path = getIntent().getStringExtra("path");
        String target = getIntent().getStringExtra("target");
        if (target == null) {
            target = "home";
        }

        if (path == null || path.trim().isEmpty()) {
            Log.e(TAG, "No path provided");
            finish();
            return;
        }

        try {
            File file = new File(path);
            if (!file.exists()) {
                Log.e(TAG, "File does not exist: " + path);
                finish();
                return;
            }

            Bitmap bitmap = BitmapFactory.decodeFile(file.getAbsolutePath());
            if (bitmap == null) {
                Log.e(TAG, "Failed to decode image at: " + path);
                finish();
                return;
            }

            WallpaperManager wm = WallpaperManager.getInstance(this);

            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
                int which;
                switch (target.toLowerCase()) {
                    case "lock":
                        which = WallpaperManager.FLAG_LOCK;
                        break;
                    case "both":
                        which = WallpaperManager.FLAG_SYSTEM | WallpaperManager.FLAG_LOCK;
                        break;
                    case "home":
                    default:
                        which = WallpaperManager.FLAG_SYSTEM;
                        break;
                }
                wm.setBitmap(bitmap, null, true, which);
            } else {
                // Pre-N, no flags API; this sets the home screen wallpaper
                wm.setBitmap(bitmap);
            }

            Log.i(TAG, "Wallpaper set successfully from: " + path + " target=" + target);
        } catch (Exception e) {
            Log.e(TAG, "Error setting wallpaper", e);
        } finally {
            finish();
        }
    }
}
