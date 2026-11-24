# docs: test="docs"
from cua_sandbox import Image, RegistrySecret

print(Image.from_registry("python:3.12-slim").to_dict()["registry"])
print(repr(RegistrySecret("robot", "s3cr3t", registry="registry.example.com")))  # the password never shows
