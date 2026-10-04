// docs: test="kotlin"
import ai.cua.sdk.Cua
import ai.cua.sdk.CuaConfig

val spaces = Cua.embedded(CuaConfig()).spaces()
val info = spaces.add("http://10.0.0.5:3211", "TOKEN", "lab") // a machine running cua-spacesd
val space = spaces.space(info.id)
val out = space.bash("echo hi", null)
println("${info.provider} ${out.stdout} ${out.exitCode}")
