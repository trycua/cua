package ai.cua.android.demo

import android.content.ContentProvider
import android.content.ContentValues
import android.database.Cursor
import android.database.MatrixCursor
import android.net.Uri
import org.json.JSONObject

/** Read-only evidence for this synthetic fixture. No production data belongs here. */
class StateProvider : ContentProvider() {
    override fun onCreate() = true
    override fun getType(uri: Uri) = "application/json"
    override fun query(uri: Uri, projection: Array<out String>?, selection: String?, selectionArgs: Array<out String>?, sortOrder: String?): Cursor {
        val state = synchronized(StateProvider::class.java) {
            val file = java.io.File(requireNotNull(context).filesDir, "state.json")
            if (file.exists()) file.readText() else JSONObject().put("status", "not_started").toString()
        }
        return MatrixCursor(arrayOf("json")).apply { addRow(arrayOf(state)) }
    }
    override fun insert(uri: Uri, values: ContentValues?): Uri? = throw UnsupportedOperationException("Read-only fixture")
    override fun update(uri: Uri, values: ContentValues?, selection: String?, selectionArgs: Array<out String>?) = throw UnsupportedOperationException("Read-only fixture")
    override fun delete(uri: Uri, selection: String?, selectionArgs: Array<out String>?) = throw UnsupportedOperationException("Read-only fixture")
}
