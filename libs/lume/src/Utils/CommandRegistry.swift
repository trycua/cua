import ArgumentParser

enum CommandRegistry {
    static var allCommands: [ParsableCommand.Type] {
        [
            Create.self,
            Pull.self,
            Push.self,
            Images.self,
            Clone.self,
            Get.self,
            Set.self,
            List.self,
            Run.self,
            Stop.self,
            IPSW.self,
            Serve.self,
            Delete.self,
            Prune.self,
            Config.self,
            Logs.self,
            Setup.self,
            DumpDocs.self,
        ]
    }
}
