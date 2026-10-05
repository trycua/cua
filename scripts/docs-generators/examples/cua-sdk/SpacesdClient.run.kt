// docs: test="kotlin"
import ai.cua.sdk.Cua
import ai.cua.sdk.CuaConfig
import ai.cua.sdk.SpacesdCommand

val guest = Cua.embedded(CuaConfig()).spacesd("http://10.0.0.5:3211", "TOKEN") // or: sandbox.spacesd(null)
val out = guest.run(SpacesdCommand(program = "echo", args = listOf("hello")))
println("${out.exit.success} ${out.stdout.decodeToString()}")
