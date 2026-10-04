// docs: test="kotlin"
import ai.cua.sdk.Cua
import ai.cua.sdk.CuaConfig

val guest = Cua.embedded(CuaConfig()).spacesd("http://10.0.0.5:3211", "TOKEN")
val shot = guest.screenshot(null) // PNG by default
guest.click(shot.width.toDouble() / 2, shot.height.toDouble() / 2)
println("${shot.width} ${shot.image.size}")
