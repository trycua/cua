// docs: test="kotlin"
import ai.cua.sdk.Cua
import ai.cua.sdk.CuaConfig
import ai.cua.sdk.Image
import ai.cua.sdk.ReadinessProbe
import ai.cua.sdk.SandboxCreateOptions

val cua = Cua.embedded(CuaConfig())
val sb = cua.sandboxes().create(
    SandboxCreateOptions(
        on = "local", // or "cloud"
        image = Image.linux(),
        name = "dev",
        waitFor = listOf(ReadinessProbe(service = "env")), // until its spacesd answers
    )
)
val guest = sb.spacesd(null)
println(guest.sh("uname -a", null).stdout.decodeToString())
sb.delete()
