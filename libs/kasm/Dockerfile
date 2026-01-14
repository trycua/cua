FROM kasmweb/core-ubuntu-jammy:1.17.0
USER root

ENV HOME=/home/kasm-default-profile
ENV STARTUPDIR=/dockerstartup
ENV INST_SCRIPTS=$STARTUPDIR/install
WORKDIR $HOME

######### Customize Container Here ###########

# Installing python, pip, and libraries (but don't change default yet)
RUN apt-get update && \
    apt install -y wget build-essential libncursesw5-dev libssl-dev \
    libsqlite3-dev tk-dev libgdbm-dev libc6-dev libbz2-dev libffi-dev zlib1g-dev && \
    add-apt-repository ppa:deadsnakes/ppa && \
    apt-get update && \
    apt install -y python3.12 python3.12-dev python3.12-tk \
    gnome-screenshot wmctrl ffmpeg socat xclip curl && \
    apt-get clean && rm -rf /var/lib/apt/lists/*

# Install Firefox (do this BEFORE changing default Python, as add-apt-repository needs system Python)
ENV DEBIAN_FRONTEND=noninteractive \
    INST_DIR=$STARTUPDIR/install
COPY ./src/ $INST_DIR
RUN bash ${INST_DIR}/ubuntu/install/firefox/install_firefox.sh

# Now set Python 3.12 as the default python3
RUN update-alternatives --install /usr/bin/python3 python3 /usr/bin/python3.12 1 && \
    update-alternatives --set python3 /usr/bin/python3.12

# Install pip for Python 3.12
RUN curl -sS https://bootstrap.pypa.io/get-pip.py | python3.12

# Install UV for fast package management
RUN python3.12 -m pip install uv

# Install cua-computer-server using UV
RUN uv pip install --system cua-computer-server

# Disable SSL requirement
RUN sed -i 's/require_ssl: true/require_ssl: false/g' /usr/share/kasmvnc/kasmvnc_defaults.yaml
RUN sed -i 's/-sslOnly//g' /dockerstartup/vnc_startup.sh

# Running the python script on startup
RUN echo "/usr/bin/python3 -m computer_server" > $STARTUPDIR/custom_startup.sh \
    && chmod +x $STARTUPDIR/custom_startup.sh

# Enable sudo support for kasm-user
RUN echo "kasm-user:password" | chpasswd
RUN usermod -aG sudo kasm-user
RUN echo "kasm-user ALL=(ALL) NOPASSWD:ALL" >> /etc/sudoers

######### End Customizations ###########

RUN chown 1000:0 $HOME
RUN $STARTUPDIR/set_user_permission.sh $HOME
ENV HOME=/home/kasm-user
WORKDIR $HOME
RUN mkdir -p $HOME && chown -R 1000:0 $HOME

USER 1000