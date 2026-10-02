# docs: test="docs"
import cua

print([cua.parse_sandbox_ref(r).id for r in ("space://fleet/ns/box", "url:10.0.0.5:3211", "box")])
# ['cloud:box', 'direct:10.0.0.5:3211', 'box']
