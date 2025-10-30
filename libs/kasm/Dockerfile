FROM kasmweb/core-ubuntu-jammy:1.17.0
USER root

ENV HOME=/home/kasm-default-profile
ENV STARTUPDIR=/dockerstartup
ENV INST_SCRIPTS=$STARTUPDIR/install
WORKDIR $HOME

######### Customize Container Here ###########

# Installing python, pip, and libraries
RUN apt-get update
RUN apt install -y wget build-essential libncursesw5-dev libssl-dev \
libsqlite3-dev tk-dev libgdbm-dev libc6-dev libbz2-dev libffi-dev zlib1g-dev
RUN add-apt-repository ppa:deadsnakes/ppa
RUN apt install -y python3.11 python3-pip python3-tk python3-dev \
gnome-screenshot wmctrl ffmpeg socat xclip

RUN pip install cua-computer-server

# Install Firefox
ENV DEBIAN_FRONTEND=noninteractive \
    INST_DIR=$STARTUPDIR/install
COPY ./src/ $INST_DIR
RUN bash ${INST_DIR}/ubuntu/install/firefox/install_firefox.sh

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