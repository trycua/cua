package mcp.fetch

default allow = false

# Loopback only: the experiment's static bundle host and mock Cyclops backend.
allow if {
    input.url_parsed.host == "127.0.0.1"
}
