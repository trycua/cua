# docs: test="docs"
import cua

c = cua.embedded()  # the SDK runs in this process; cua.connect() uses a running `cua daemon`
print(c.mode())
