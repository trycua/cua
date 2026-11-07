#!/usr/bin/env bash
set -euo pipefail

commit=13f18b2cb7286fb54f87daf571a031aa6af3d8f0
source_dir=/usr/share/omarchy
target_user=${OMARCHY_USER:-omarchy}
target_home=$(getent passwd "${target_user}" | cut -d: -f6)

[[ ${EUID} -eq 0 ]] || {
  echo 'Run this script with sudo.' >&2
  exit 1
}
[[ -n ${target_home} && -d ${target_home} ]] || {
  printf 'User %s does not exist.\n' "${target_user}" >&2
  exit 1
}

pacman -Syu --noconfirm

# This is the native ARM64 subset used in the tested VM. The missing upstream
# packages are recorded at the end of the script.
packages=(
  alsa-utils bash-completion bat bluez bluez-utils brightnessctl btop chromium
  cups docker docker-buildx docker-compose dosfstools eza fd ffmpegthumbnailer
  foot fzf git gnome-keyring grim gvfs-mtp gvfs-nfs gvfs-smb hyprland
  hyprland-guiutils hyprpicker hyprsunset imagemagick imv jq less libsecret
  lua51 luarocks man-db mesa mesa-utils nautilus nautilus-python neovim
  noto-fonts noto-fonts-cjk noto-fonts-emoji pamixer pipewire pipewire-alsa
  pipewire-pulse plocate power-profiles-daemon python-gobject quickshell ripgrep
  sddm slurp socat starship sushi tmux tree-sitter-cli
  ttf-jetbrains-mono-nerd udiskie ufw unzip uwsm vulkan-tools vulkan-virtio
  whois wireplumber wl-clipboard wtype xdg-desktop-portal-gtk
  xdg-desktop-portal-hyprland xournalpp yt-dlp zbar zoxide
)
pacman -S --needed --noconfirm "${packages[@]}"

if [[ ! -d ${source_dir}/.git ]]; then
  git clone https://github.com/basecamp/omarchy.git "${source_dir}"
fi
git -C "${source_dir}" fetch --tags --force
git -C "${source_dir}" checkout --detach "${commit}"
[[ $(git -C "${source_dir}" rev-parse HEAD) == "${commit}" ]]

for command in "${source_dir}"/bin/*; do
  [[ -f ${command} && -x ${command} ]] || continue
  ln -sfn "${command}" "/usr/local/bin/$(basename "${command}")"
done

install -d /usr/local/share/wayland-sessions /usr/share/sddm/themes /etc/sddm.conf.d
install -m644 "${source_dir}/default/wayland-sessions/omarchy.desktop" \
  /usr/local/share/wayland-sessions/omarchy.desktop
cp -a "${source_dir}/default/sddm/omarchy" /usr/share/sddm/themes/
install -m644 "${source_dir}/default/sddm/hyprland.lua" /usr/share/sddm/hyprland.lua
cat >/etc/sddm.conf.d/10-omarchy.conf <<SDDM
[Autologin]
User=${target_user}
Session=omarchy.desktop

[Theme]
Current=omarchy
SDDM

install -d -o "${target_user}" -g "${target_user}" \
  "${target_home}/.config" "${target_home}/.local/share/applications"
cp -a "${source_dir}/config/." "${target_home}/.config/"
cp -a "${source_dir}/applications/." "${target_home}/.local/share/applications/"
chown -R "${target_user}:${target_user}" "${target_home}"

# Apple Virtualization.framework exposes a display device, but not an
# accelerated Linux render node. Force the compositor onto its software path.
install -d /etc/environment.d
cat >/etc/environment.d/90-omarchy-lume.conf <<'ENV'
LIBGL_ALWAYS_SOFTWARE=1
WLR_RENDERER_ALLOW_SOFTWARE=1
AQ_NO_MODIFIERS=1
ENV

usermod -aG docker "${target_user}"
systemctl enable sddm bluetooth cups docker
systemctl set-default graphical.target
printf '%s\n' "${commit}" >/etc/omarchy-source-commit

cat >/var/log/omarchy-v4.0.1-unavailable-arm64-packages.txt <<'PACKAGES'
aether
asdcontrol
cliamp
dotnet-runtime
herdr
hyprland-preview-share-picker
localsend
mise-bin
nvim
obs-studio
obsidian
omacalc
omacut
omawrite
omarchy-nvim
pinta
ttfx
qemu-user-static-binfmt
tensaku
tobi-try
ttf-ia-writer
ttf-jetbrains-mono-nerd-basic
tzupdate
ufw-docker
xdg-terminal-exec
yaru-icon-theme
yay
PACKAGES

echo 'Omarchy v4.0.1 ARM64 compatibility installation complete.'
echo 'Reboot to start the Omarchy Wayland session.'
