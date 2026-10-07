# docs: test="docs"
import cua

print([cua.qualify_sandbox_ref("box", True), cua.qualify_sandbox_ref("box", False), cua.qualify_sandbox_ref("box", None)])
# ['local:box', 'cloud:box', 'box']
