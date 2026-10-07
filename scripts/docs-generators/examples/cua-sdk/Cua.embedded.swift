// docs: test="swift"
import Cua

let cua = try Cua.embedded()  // the SDK runs in this process
print(cua.mode())
