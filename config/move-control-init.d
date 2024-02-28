#!/bin/sh
#
# Required-Start: jackd
#
# Called at system startup
#
# Attempt to start move-control

cmd=/data/UserData/rnbo/move-control
name=rnbo-move-control

if [ "$1" = "start" ]; then
    if [ -e $cmd ]; then
        export LD_LIBRARY_PATH=$LD_LIBRARY_PATH:/data/UserData/jack2/lib/
        start-stop-daemon --start --name $name -c ableton -b -x $cmd
    fi
elif [ "$1" = "stop" ]; then
    start-stop-daemon --stop --name $name
fi
