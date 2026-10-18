// docs: test="kotlin"
import ai.cua.sdk.Cua
import ai.cua.sdk.CuaConfig
import ai.cua.sdk.SpacesdCommand

val guest = Cua.embedded(CuaConfig()).spacesd("http://10.0.0.5:3211", "TOKEN")
val proc = guest.spawn(SpacesdCommand(program = "cat", stdin = true))
proc.writeStdin("xyz".encodeToByteArray())
proc.closeStdin()
println(proc.wait().stdout.decodeToString())
