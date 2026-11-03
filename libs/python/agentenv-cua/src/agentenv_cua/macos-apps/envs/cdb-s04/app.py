"""The bench's CDB-S04, linked support diagnosis, as one computer app on macOS: the bench's Rillwatch Cases and Margin
Lantern Manual Electron apps, its Morrowglass Archive service on Node, and Chrome for Testing on that service, all on its
workspace, ~/cdb-s04."""

from agentenv_protocol.computer import computer_app

import bench_macos as bench


@computer_app(supports=bench.SUPPORTS)
@bench.macos(
    **bench.task_install(
        workspace="cdb-s04", first_window="Rillwatch Cases", node=True, browser=True
    )
)
class CdbS04(bench.BenchTaskApp):
    pass
