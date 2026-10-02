// docs: test="kotlin"
import ai.cua.sdk.Cua
import ai.cua.sdk.CuaConfig
import ai.cua.sdk.CuaException

val sb = Cua.embedded(CuaConfig()).sandboxes().connectUrl("http://10.0.0.5:3211", "wrong-token", null)
try {
    sb.spacesd(5000u)
} catch (e: CuaException.Unauthenticated) { // one subclass; any other CuaException propagates
    println("rejected: ${e.message}")
}
