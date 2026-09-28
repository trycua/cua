package ai.cua.driver

import android.content.ContentProvider
import android.content.ContentValues
import android.content.pm.PackageManager
import android.database.Cursor
import android.net.Uri
import android.os.Binder
import android.os.Bundle
import android.os.IBinder
import android.os.Process

/** Exchanges only a shell-owned Binder handle with applications signed like this runtime. */
class BridgeProvider : ContentProvider() {
    private val lock = Any()
    private var bridge: IBinder? = null
    private var deathRecipient: IBinder.DeathRecipient? = null

    override fun onCreate() = true

    override fun call(method: String, arg: String?, extras: Bundle?): Bundle {
        val uid = Binder.getCallingUid()
        return when (method) {
            "register" -> {
                checkCaller(uid == 2000)
                val incoming = requireNotNull(extras?.getBinder("binder")) { "Missing bridge binder" }
                synchronized(lock) {
                    val recipient = IBinder.DeathRecipient {
                        synchronized(lock) {
                            if (bridge === incoming) {
                                bridge = null
                                deathRecipient = null
                            }
                        }
                    }
                    incoming.linkToDeath(recipient, 0)
                    deathRecipient?.let { bridge?.unlinkToDeath(it, 0) }
                    bridge = incoming
                    deathRecipient = recipient
                }
                Bundle()
            }
            "get" -> {
                val manager = requireNotNull(context).packageManager
                checkCaller(manager.checkSignatures(uid, Process.myUid()) == PackageManager.SIGNATURE_MATCH)
                synchronized(lock) { Bundle().apply { putBinder("binder", bridge) } }
            }
            else -> throw UnsupportedOperationException("Unknown bridge method")
        }
    }

    private fun checkCaller(allowed: Boolean) {
        if (!allowed) throw SecurityException("Caller is not authorized for the runtime bridge")
    }

    override fun query(uri: Uri, projection: Array<out String>?, selection: String?,
                       selectionArgs: Array<out String>?, sortOrder: String?): Cursor? =
        throw UnsupportedOperationException("Bridge has no URI data")
    override fun getType(uri: Uri): String? = throw UnsupportedOperationException("Bridge has no URI data")
    override fun insert(uri: Uri, values: ContentValues?): Uri? =
        throw UnsupportedOperationException("Bridge has no URI data")
    override fun delete(uri: Uri, selection: String?, selectionArgs: Array<out String>?): Int =
        throw UnsupportedOperationException("Bridge has no URI data")
    override fun update(uri: Uri, values: ContentValues?, selection: String?, selectionArgs: Array<out String>?): Int =
        throw UnsupportedOperationException("Bridge has no URI data")
}
