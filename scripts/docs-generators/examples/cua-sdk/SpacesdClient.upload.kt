// docs: test="kotlin"
import ai.cua.sdk.Cua
import ai.cua.sdk.CuaConfig

val guest = Cua.embedded(CuaConfig()).spacesd("http://10.0.0.5:3211", "TOKEN")
val sent = guest.upload("/tmp/note.txt", "hi from the host".encodeToByteArray(), null)
val back = guest.download("/tmp/note.txt")
println("${sent.size} ${back.decodeToString()}")
