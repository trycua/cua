class BaseDioramaHandler:
    """Base Diorama handler for unsupported OSes."""

    async def diorama_cmd(self, action: str, arguments: dict = None) -> dict:
        return {"success": False, "error": "Diorama is not supported on this OS yet."}
