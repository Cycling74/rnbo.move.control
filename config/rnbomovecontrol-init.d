#!/bin/sh
#
# Required-Start: jackd
#
# Called at system startup
#
# Attempt to start rnbomovecontrol

cmd=/data/UserData/rnbo/rnbomovecontrol
name=rnbomovecontrol

if [ "$1" = "start" ]; then
    if [ -e $cmd ]; then
        export LD_LIBRARY_PATH=$LD_LIBRARY_PATH:/data/UserData/jack2/lib/
        start-stop-daemon --start --name $name -c ableton -b $cmd
    fi
elif [ "$1" = "stop" ]; then
    start-stop-daemon --stop --name $name
fi
