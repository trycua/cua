// docs: test="kotlin"
import ai.cua.sdk.Cua
import ai.cua.sdk.CuaConfig

val cua = Cua.embedded(CuaConfig()) // the SDK runs in this process
println(cua.mode())
