#!/bin/bash
# This script is run regularly; eg. every 5 minutes.

mkdir -p logs
mkdir -p executed_tests
mkdir -p testcases
mkdir -p stalled_testcases
echo -1 > last_logfile


# ------ Delete logs with no conflicts ------
# This is necessary otherwise we run out of disc space.

files="/home/ubuntu/logs/*.log"
for file in $files
do 
    ok="$(cat $file | grep "test result: ok. 1 passed; 0 failed;")"
    if [ -z "$ok" ] ; then
        echo "$file..........IN PROGRESS."
    else
        echo "$file..........OK."
        rm $file
    fi
done


# ------ Restart stalled processes ------

# Get persisted state.
test -f last_logfile || touch last_logfile
last_logfile_name=$(cat last_logfile)

#test -f last_testcase || touch last_testcase
#last_testcase_number=$(cat last_testcase)

# Get current state.
current_logfile_name=$(ls logs/ -Art | tail -n 1)
#current_testcase_number=$(cat logs/${current_logfile_name} | grep "TEST CASE")

# Check whether the test is stalled.
#last_state="${last_logfile_name}-${last_testcase_number}"
#current_state="${current_logfile_name}-${current_testcase_number}"
last_state=$last_logfile_name
current_state=$current_logfile_name
if [ "$last_state" = "$current_state" ]
then
    # Kill the current test.
    tmux kill-server

    # Move out stalled testcases.
    testcase_file=$(echo "$last_logfile_name" | cut -f 1 -d '.')
    testcase_file="$testcase_file.bin"
    mv "testcases/${testcase_file}" ./stalled_testcases/
    mv "logs/${current_logfile_name}" ./stalled_testcases/

    # Restart the test.
    tmux new-session -d -s "twins" ./twins-aws-run.sh

    # Clear file.
    echo -1 > last_logfile
    #echo -1 > last_testcase
else
    echo $current_logfile_name > last_logfile
    #echo $current_testcase_number > last_testcase
fi
