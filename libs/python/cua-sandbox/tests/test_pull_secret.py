"""The ECR pull secret must only ride on images that need it.

Attaching `imagePullSecret: ecr-credentials` to a public image is not merely
redundant — the gateway's admission policy reads its presence as "enforce the
private-registry allowlist", so a public image the cluster can pull anonymously
was refused with `403 k8s request is not allowed`. Proven with identical request
bodies differing only in the image: ghcr.io -> 403, public.ecr.aws -> 201.
"""

import pytest
from cua_sandbox import Image
from cua_sandbox.transport.fleet_cloud import (
    FleetCloudTransport,
    _needs_ecr_pull_secret,
)

PRIVATE = "123456789012.dkr.ecr.us-west-2.amazonaws.com/example-workspace:main-bac7daa3"
PUBLIC_ECR = "public.ecr.aws/k5j5w0x5/cua-windows-2022:main-bac7daa3"


@pytest.mark.parametrize(
    "ref,expected",
    [
        (PRIVATE, True),
        ("123456789012.dkr.ecr.eu-central-1.amazonaws.com/x:1", True),
        (PUBLIC_ECR, False),
        ("ghcr.io/trycua/minecraft-workspace:latest", False),
        ("quay.io/org/image:tag", False),
        ("ubuntu:22.04", False),
        (None, False),
        ("", False),
    ],
)
def test_only_private_ecr_needs_the_secret(ref, expected):
    assert _needs_ecr_pull_secret(ref) is expected


def test_public_image_template_carries_no_pull_secret():
    """A public image must not be forced into the allowlist branch."""
    request = FleetCloudTransport(image=Image.windows(), name="demo")._template_request()
    template = request.spec.vm_template
    assert template.container_disk_image == PUBLIC_ECR
    assert not getattr(template, "image_pull_secret", None)


def test_private_registry_image_still_carries_the_secret():
    request = FleetCloudTransport(
        image=Image.from_registry(PRIVATE), name="demo"
    )._template_request()
    assert request.spec.vm_template.image_pull_secret == "ecr-credentials"


def test_a_public_non_ecr_registry_is_usable():
    """The case the Minecraft guide needs: a containerDisk pushed to ghcr.io."""
    ref = "ghcr.io/trycua/minecraft-workspace:latest"
    request = FleetCloudTransport(image=Image.from_registry(ref), name="demo")._template_request()
    template = request.spec.vm_template
    assert template.container_disk_image == ref
    assert not getattr(template, "image_pull_secret", None)
