"""The bench's CDB-S02, a conflicting change to replan, as one computer app on macOS: Copperfield Mail, Signal Quay Chat and Daymark Calendar, the three windows of the bench's lighthouse-desk Electron app, and LibreOffice Calc on the task's workbook, all on its workspace, ~/cdb-s02."""

from agentenv_protocol.computer import computer_app

import bench_macos as bench


@computer_app(supports=bench.SUPPORTS)
@bench.macos(**bench.task_install(workspace="cdb-s02", first_window="Copperfield Mail"))
class CdbS02(bench.BenchTaskApp):
    pass
