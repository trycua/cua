"""Constants of the template builder (same names and values as e2b's).

The e2b SDK is MIT-licensed; this module matches its public API.
"""

# Last step of a build: runs after every user instruction.
FINALIZE_STEP_NAME = "finalize"

# First step of a build: sets up the base image.
BASE_STEP_NAME = "base"

# Whether COPY follows symbolic links by default (False: they are copied as links).
RESOLVE_SYMLINKS = False

# Whether the archive of a COPY is gzipped by default.
GZIP = True

# Default timeout (seconds) for handing a build-context archive to the build.
FILE_UPLOAD_TIMEOUT_SECONDS = 3600
