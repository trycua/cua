# Cua XFCE Container

Multi-architecture XFCE desktop container for Computer-Using Agents.

`trycua/cua-xfce` extends the application-neutral `trycua/xfce-cua` desktop
with `cua-computer-server`, preserving the local `cua-sandbox` DockerRuntime
contract:

- computer-server API on port 8000
- noVNC on port 6901
- native `linux/amd64` and `linux/arm64` images

The container relies on Docker's existing network configuration and does not
run a DHCP client against Docker-managed interfaces.

```bash
docker run --rm -p 8000:8000 -p 6901:6901 trycua/cua-xfce:latest
curl --fail http://127.0.0.1:8000/status
```
