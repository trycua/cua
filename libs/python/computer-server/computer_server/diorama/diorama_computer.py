import asyncio


class DioramaComputer:
    """
    A minimal Computer-like interface for Diorama, compatible with ComputerAgent.
    Implements _initialized, run(), and __aenter__ for agent compatibility.
    """

    def __init__(self, diorama):
        """
        Initialize the DioramaComputer with a diorama instance.

        Args:
            diorama: The diorama instance to wrap with a computer-like interface.
        """
        self.diorama = diorama
        self.interface = self.diorama.interface
        self._initialized = False

    async def __aenter__(self):
        """
        Async context manager entry method for compatibility with ComputerAgent.

        Ensures an event loop is running and marks the instance as initialized.
        Creates a new event loop if none is currently running.

        Returns:
            DioramaComputer: The initialized instance.
        """
        # Ensure the event loop is running (for compatibility)
        try:
            asyncio.get_running_loop()
        except RuntimeError:
            asyncio.set_event_loop(asyncio.new_event_loop())
        self._initialized = True
        return self

    async def run(self):
        """
        Run method stub for compatibility with ComputerAgent interface.

        Ensures the instance is initialized before returning. If not already
        initialized, calls __aenter__ to perform initialization.

        Returns:
            DioramaComputer: The initialized instance.
        """
        # This is a stub for compatibility
        if not self._initialized:
            await self.__aenter__()
        return self
