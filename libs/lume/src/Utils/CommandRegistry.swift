import ArgumentParser

enum CommandRegistry {
    static var allCommands: [ParsableCommand.Type] {
        [
            Create.self,
            Pull.self,
            Push.self,
            Convert.self,
            Images.self,
            Clone.self,
            Get.self,
            Set.self,
            List.self,
            Run.self,
            Attach.self,
            Stop.self,
            Shutdown.self,
            Restart.self,
            SSH.self,
            Sip.self,
            IPSW.self,
            Serve.self,
            Delete.self,
            Prune.self,
            Config.self,
            Logs.self,
            CheckUpdate.self,
            Update.self,
            Setup.self,
            DumpDocs.self,
        ]
    }
}
