"""The bench's CDB-S03, invoice reconciliation, as one computer app on macOS: the bench's Ledger Lantern Mail Electron app, LibreOffice Calc on the task's workbook and GnuCash on its book, all on its workspace, ~/cdb-s03."""

from agentenv_protocol.computer import computer_app

import bench_macos as bench


@computer_app(supports=bench.SUPPORTS)
@bench.macos(**bench.task_install(workspace="cdb-s03", first_window="Ledger Lantern Mail"))
class CdbS03(bench.BenchTaskApp):
    pass
