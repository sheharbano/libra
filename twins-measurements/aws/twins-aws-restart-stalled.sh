#!/bin/bash
# This script is run regularly; eg. every 5 minutes.

mkdir -p logs

# This file holds the previous size of the log file.
test -f log_size || touch log_size

# Find the current log file.
logfile=$(ls logs/ -Art | tail -n 1)

# Find the current size of the log file.
size=$(du -b "logs/${logfile}" | cut -f 1)

# Get the previous
previous_size=$(cat log_size)

# Check whether the test is stalled.
if [[ $previous_size -eq $size ]]
then
    # Kill the current test.
    tmux kill-server

    # Restart the test.
    tmux new-session -d -s "twins" ./twins-aws-run.sh

    # Clear file.
    echo -1 > log_size
else
    echo $size > log_size
fi
