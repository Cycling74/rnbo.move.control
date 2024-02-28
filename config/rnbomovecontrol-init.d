#!/bin/sh
#
# Required-Start: jackd
#
# Called at system startup
#
# Attempt to start rnbomovecontrol

cmd=/data/UserData/rnbo/rnbomovecontrol
pidfile=/tmp/rnbomovecontrol.pid

if [ "$1" = "start" ]; then
    if [ -e $cmd ]; then
        export LD_LIBRARY_PATH=$LD_LIBRARY_PATH:/data/UserData/jack2/lib/
        start-stop-daemon --start --pidfile $pidfile --make-pidfile -c ableton -b -x $cmd
    fi
elif [ "$1" = "stop" ]; then
    start-stop-daemon --stop --pidfile $pidfile
fi
