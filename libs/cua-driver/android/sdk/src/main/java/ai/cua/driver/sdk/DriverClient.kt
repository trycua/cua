package ai.cua.driver.sdk

import android.content.Context
import android.net.LocalSocket
import android.net.LocalSocketAddress
import android.net.Uri
import android.os.IBinder
import android.os.Looper
import android.os.Parcel
import android.os.ParcelFileDescriptor
import android.os.SystemClock
import android.system.ErrnoException
import android.system.Os
import android.system.OsConstants
import android.system.StructPollfd
import org.json.JSONObject
import java.io.ByteArrayOutputStream
import java.io.FileDescriptor
import java.net.SocketTimeoutException
import java.util.UUID

/** Experimental blocking transport. Call from a worker, never the UI thread. */
class DriverClient(private val context: Context? = null) {
    fun call(operation: String, sessionId: String? = null, params: JSONObject = JSONObject(),
             requestId: String = UUID.randomUUID().toString()): JSONObject {
        check(Looper.myLooper() != Looper.getMainLooper()) { "Driver calls require a worker thread" }
        val request = JSONObject().put("contract_version", VERSION)
            .put("request_id", requestId).put("operation", operation)
            .put("params", params)
        if (sessionId != null) request.put("session_id", sessionId)
        return try { if (context == null) exchange(request) else exchangeBinder(context, request) }
        catch (error: Exception) { throw UncertainRequestException(requestId, error) }
    }

    class UncertainRequestException(val requestId: String, cause: Exception) :
        Exception("Request $requestId completion is uncertain; do not blindly retry", cause)

    companion object {
        const val VERSION = "cua.android.v0"
        const val SOCKET = "cua_driver_android_v0"
        const val MAX_BYTES = 32 * 1024 * 1024
        const val BINDER_DESCRIPTOR = "ai.cua.driver.v0"
        const val BRIDGE_AUTHORITY = "ai.cua.driver.runtime.bridge"

        private fun exchangeBinder(context: Context, request: JSONObject, timeoutMs: Int = 10000): JSONObject {
            require(timeoutMs in 1..10000)
            val raw = request.toString()
            require(raw.toByteArray(Charsets.UTF_8).size <= 65536) { "Request exceeds 64 KiB" }
            val deadline = SystemClock.elapsedRealtime() + timeoutMs
            val bridge = context.contentResolver.call(
                Uri.parse("content://$BRIDGE_AUTHORITY"), "get", null, null
            )?.getBinder("binder") ?: error("Runtime bridge is unavailable")
            val data = Parcel.obtain()
            val reply = Parcel.obtain()
            try {
                data.writeInterfaceToken(BINDER_DESCRIPTOR)
                data.writeString(raw)
                check(bridge.transact(IBinder.FIRST_CALL_TRANSACTION, data, reply, 0)) { "Bridge transaction unsupported" }
                reply.readException()
                val pipe = requireNotNull(reply.readTypedObject(ParcelFileDescriptor.CREATOR)) { "Missing response pipe" }
                val response = pipe.use { JSONObject(readPipeBounded(it.fileDescriptor, deadline)) }
                check(response.getString("contract_version") == VERSION) { "Incompatible runtime" }
                check(response.getString("request_id") == request.getString("request_id")) { "Request mismatch" }
                return response
            } finally {
                data.recycle()
                reply.recycle()
            }
        }

        private fun readPipeBounded(descriptor: FileDescriptor, deadline: Long): String {
            val out = ByteArrayOutputStream()
            val bytes = ByteArray(8192)
            val pollfd = StructPollfd().apply {
                fd = descriptor
                events = OsConstants.POLLIN.toShort()
            }
            while (true) {
                val remaining = deadline - SystemClock.elapsedRealtime()
                if (remaining <= 0) throw SocketTimeoutException("Runtime response deadline exceeded")
                try {
                    pollfd.revents = 0
                    if (Os.poll(arrayOf(pollfd), remaining.toInt()) == 0) {
                        throw SocketTimeoutException("Runtime response deadline exceeded")
                    }
                    check(pollfd.revents.toInt() and (OsConstants.POLLERR or OsConstants.POLLNVAL) == 0) {
                        "Response pipe failed"
                    }
                    // POLLHUP can accompany the last readable bytes; drain them before treating EOF as failure.
                    val count = Os.read(descriptor, bytes, 0, bytes.size)
                    check(count > 0) { "Connection ended before response" }
                    val newline = (0 until count).firstOrNull { bytes[it] == 10.toByte() }
                    val length = newline ?: count
                    check(out.size() + length <= MAX_BYTES) { "Message too large" }
                    out.write(bytes, 0, length)
                    if (newline != null) return out.toString("UTF-8")
                } catch (error: ErrnoException) {
                    if (error.errno != OsConstants.EINTR) throw error
                }
            }
        }

        fun exchange(request: JSONObject, timeoutMs: Int = 10000): JSONObject = LocalSocket().use { socket ->
            require(timeoutMs in 1..10000)
            socket.connect(LocalSocketAddress(SOCKET, LocalSocketAddress.Namespace.ABSTRACT))
            check(socket.peerCredentials.uid == 2000) { "Runtime must be the authorized shell helper" }
            socket.soTimeout = timeoutMs
            val bytes = request.toString().toByteArray(Charsets.UTF_8)
            require(bytes.size <= 65536) { "Request exceeds 64 KiB" }
            socket.outputStream.write(bytes)
            socket.outputStream.write(10)
            socket.outputStream.flush()
            val response = JSONObject(readLineBounded(socket.inputStream, MAX_BYTES))
            check(response.getString("contract_version") == VERSION) { "Incompatible runtime" }
            check(response.getString("request_id") == request.getString("request_id")) { "Request mismatch" }
            response
        }

        fun readLineBounded(input: java.io.InputStream, limit: Int): String {
            val out = java.io.ByteArrayOutputStream()
            val buffered = input.buffered()
            while (true) {
                val value = buffered.read()
                check(value != -1) { "Connection ended before response" }
                if (value == 10) return out.toString("UTF-8")
                check(out.size() < limit) { "Message too large" }
                out.write(value)
            }
        }
    }
}
