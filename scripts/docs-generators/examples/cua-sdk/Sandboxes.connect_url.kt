// docs: test="kotlin"
import ai.cua.sdk.Cua
import ai.cua.sdk.CuaConfig
import ai.cua.sdk.SpacesdCommand

val cua = Cua.embedded(CuaConfig())
val sb = cua.sandboxes().connectUrl("http://10.0.0.5:3211", "TOKEN", "dev") // your spacesd's address and token
val guest = sb.spacesd(null)
val out = guest.run(SpacesdCommand(program = "echo", args = listOf("hi")))
println(out.stdout.decodeToString())
