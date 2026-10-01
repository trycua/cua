#!/usr/bin/env bash
set -ex
START_COMMAND="firefox"
PGREP="firefox"
export MAXIMIZE="true"
export MAXIMIZE_NAME="Mozilla Firefox"
MAXIMIZE_SCRIPT=$STARTUPDIR/maximize_window.sh
DEFAULT_ARGS=""
ARGS=${APP_ARGS:-$DEFAULT_ARGS}

options=$(getopt -o gau: -l go,assign,url: -n "$0" -- "$@") || exit
eval set -- "$options"

while [[ $1 != -- ]]; do
    case $1 in
        -g|--go) GO='true'; shift 1;;
        -a|--assign) ASSIGN='true'; shift 1;;
        -u|--url) OPT_URL=$2; shift 2;;
        *) echo "bad option: $1" >&2; exit 1;;
    esac
done
shift

# Process non-option arguments.
for arg; do
    echo "arg! $arg"
done

FORCE=$2

# run with vgl if GPU is available
if [ -f /opt/VirtualGL/bin/vglrun ] && [ ! -z "${KASM_EGL_CARD}" ] && [ ! -z "${KASM_RENDERD}" ] && [ -O "${KASM_RENDERD}" ] && [ -O "${KASM_EGL_CARD}" ] ; then
    START_COMMAND="/opt/VirtualGL/bin/vglrun -d ${KASM_EGL_CARD} $START_COMMAND"
fi

kasm_exec() {
    if [ -n "$OPT_URL" ] ; then
        URL=$OPT_URL
    elif [ -n "$1" ] ; then
        URL=$1
    fi

    # Since we are execing into a container that already has the browser running from startup,
    #  when we don't have a URL to open we want to do nothing. Otherwise a second browser instance would open.
    if [ -n "$URL" ] ; then
        /usr/bin/filter_ready
        /usr/bin/desktop_ready
        bash ${MAXIMIZE_SCRIPT} &
        $START_COMMAND $ARGS $OPT_URL
    else
        echo "No URL specified for exec command. Doing nothing."
    fi
}

kasm_startup() {
    if [ -n "$KASM_URL" ] ; then
        URL=$KASM_URL
    elif [ -z "$URL" ] ; then
        URL=$LAUNCH_URL
    fi

    if [ -z "$DISABLE_CUSTOM_STARTUP" ] ||  [ -n "$FORCE" ] ; then

        echo "Entering process startup loop"
        set +x
        while true
        do
            if ! pgrep -x $PGREP > /dev/null
            then
                /usr/bin/filter_ready
                /usr/bin/desktop_ready
                set +e
                bash ${MAXIMIZE_SCRIPT} &
                $START_COMMAND $ARGS $URL
                set -e
            fi
            sleep 1
        done
        set -x

    fi

}

if [ -n "$GO" ] || [ -n "$ASSIGN" ] ; then
    kasm_exec
else
    kasm_startup
fi
