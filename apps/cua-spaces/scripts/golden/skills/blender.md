# Blender work in a Cua Space

What changes when Blender work happens in a Space instead of on the user's own
desktop. Read this to interpret the request; the Blender MCP's own tool docs
cover how to drive the application.

A **Space** is a fresh, disposable sandbox desktop, separate from the user's
machine. Blender is installed in it and its MCP is wired up and exposed to you
as tools, so it is a workstation the agent can actually build in, not just a
viewer.

**The Space is already set up for agent work, but Blender is not on screen
until you use it.** The Space starts with a clean desktop and hot-loads
applications on demand: the Blender tools are listed and callable at all times,
and the FIRST call to one launches Blender and waits for its MCP socket before
running your command. That first call can take up to a minute. It is not a
hang, and it is not a setup step you are missing: do not launch Blender
yourself, do not retry, and do not fall back to another approach. Later calls
are immediate.

**When an asset the user asks for does not exist, author it here.** A request to
add something the project does not contain is a request to *make* it. Model it
in Blender in the Space. Do not substitute a file downloaded from the web, copied
from the user's machine, or found elsewhere on disk: that is a different result
from the one asked for, and in a Space it is also unnecessary: the tool to
build it is already wired up and one call away.

**Write output straight into the destination project.** Both applications share
one filesystem inside the Space, so export to the project's own asset directory
rather than to a temporary path and then moving it. The engine picks it up on
its next refresh.

**Keep the work inside the Space.** Assets produced here belong in the Space's
project, alongside everything else the task touches. Move results back to the
user's machine only when they explicitly ask for a file.
