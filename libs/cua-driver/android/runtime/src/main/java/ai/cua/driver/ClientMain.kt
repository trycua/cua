package ai.cua.driver

import ai.cua.driver.sdk.DriverClient
import android.util.Base64
import org.json.JSONObject

/** ADB bridge payload is base64 so it cannot become remote shell syntax. */
object ClientMain {
    @JvmStatic fun main(args: Array<String>) {
        try {
            require(args.size == 1) { "Expected one encoded request" }
            val request = JSONObject(String(Base64.decode(args[0], Base64.NO_WRAP), Charsets.UTF_8))
            val response = DriverClient.exchange(request)
            println(response)
            kotlin.system.exitProcess(response.getInt("exit_code"))
        } catch (error: Exception) {
            System.err.println("Android runtime connection failed: ${error.javaClass.simpleName}")
            kotlin.system.exitProcess(4)
        }
    }
}
