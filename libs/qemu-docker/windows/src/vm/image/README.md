Windows installation media is supplied at container runtime; it is not copied
from this source folder into the image. For first boot, mount the ISO at
`/storage/custom.iso`. See the [Windows container README](../../../README.md)
for a complete `docker run` example.

**Download Windows 11 Evaluation ISO:**

1. Visit [Microsoft Evaluation Center](https://info.microsoft.com/ww-landing-windows-11-enterprise.html)
2. Accept the Terms of Service
3. Download **Windows 11 Enterprise Evaluation (90-day trial, English, United States)** ISO file [~6GB]
4. After downloading, rename the file to `setup.iso`
5. Mount it read-only as `/storage/custom.iso` when starting the container.

For a multi-edition ISO that requires a specific image selection, mount an
answer file with an explicit `InstallFrom` selection at `/storage/custom.xml`.
