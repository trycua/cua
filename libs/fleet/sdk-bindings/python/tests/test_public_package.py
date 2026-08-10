import unittest

from fleet_sdk import (
    CreateClaimRequest,
    CreatePoolRequest,
    CyclopsClient,
    CyclopsConfiguration,
    CyclopsCredentials,
    Firmware,
    HttpHeader,
    HttpRequest,
    OsGymSandboxTemplateSpec,
    OsGymSandboxWarmPoolSpec,
    RuntimeKind,
    SandboxService,
)


class PublicPackageTest(unittest.TestCase):
    def test_fleet_sdk_exports_supported_public_symbols(self):
        self.assertIsNotNone(CyclopsClient)


if __name__ == "__main__":
    unittest.main()
