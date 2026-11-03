// docs: test="kotlin"
import ai.cua.sdk.Cua
import ai.cua.sdk.CuaConfig

val report = Cua.embedded(CuaConfig()).local().doctor()
for (check in report.checks) println("${check.name} ${check.status}")
