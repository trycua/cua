# An unmodified Daytona program: no gateway flags, no environment set up by hand.
from daytona import Daytona

d = Daytona()
sb = d.create()
print("cpu", sb.cpu, "memory", sb.memory, "disk", sb.disk, "state", str(sb.state))
r = sb.process.exec("echo plain-ok; nproc; free -m | sed -n 2p; df -BG / | tail -1")
print(r.result)
assert "plain-ok" in r.result
sb.delete()
print("PLAIN-DONE")
