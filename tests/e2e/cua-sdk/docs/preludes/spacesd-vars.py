# Hidden prelude: URL and TOKEN name a running cua-spacesd (the fixtures'
# MockServer; set by the `spacesd` prelude). Pages say where readers get them.
import os

URL = os.environ["CUA_DOCS_SPACESD_URL"]
TOKEN = os.environ["CUA_DOCS_SPACESD_TOKEN"]
