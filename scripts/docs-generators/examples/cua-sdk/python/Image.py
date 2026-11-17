# docs: test="docs"
from cua_sandbox import Image

img = Image.linux().apt_install("curl").pip_install("requests").env(DEBUG="1").expose(8080)
print(img.to_dict())
# {'os_type': 'linux', 'distro': 'ubuntu', 'version': '24.04', 'kind': None,
#  'layers': [{'type': 'apt_install', 'packages': ['curl']},
#             {'type': 'pip_install', 'packages': ['requests']}],
#  'env': {'DEBUG': '1'}, 'ports': [8080]}
