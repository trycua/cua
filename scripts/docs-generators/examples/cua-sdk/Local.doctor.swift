// docs: test="swift"
import Cua

let report = try await Cua.embedded().local().doctor()
for check in report.checks {
    print(check.name, check.status)
}
