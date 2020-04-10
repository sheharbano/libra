#!/bin/bash

# Create all possibly required files and directories.
mkdir -p logs
mkdir -p executed_tests
mkdir -p testcases
mkdir -p stalled_testcases
echo -1 > last_logfile

cd libra/consensus
source $HOME/.cargo/env

CONFIG=0
if [ "$1" != "" ]; then
	CONFIG=$1
fi

# Run all testcases from the files located in /testcases.
if [ $CONFIG == 0 ]; then
    
    testcases="/home/ubuntu/testcases/testcase*.bin"
    for file in $testcases
    do 
        name="$(basename -- $file)"
        cp $file ./to_execute.bin
        logfile="/home/ubuntu/logs/${name}.log"
        cargo xtest -p consensus execute_testcases_from_file -- --nocapture >> "${logfile}" 2>&1

        # Move execute test cases in an other directory.
        # This allow for easy restart in case a test is stalled.
        mv $file /home/ubuntu/executed_tests
    done

# Run randomly selected scenarios.
elif [ $CONFIG == 1 ]; then
    N=1
    if [ "$2" != "" ]; then
        N=$2
    fi

    mkdir -p logs
    now=$(date +"%Y-%m-%d:%H:%M:%S")

    for (( i=1; i<=$N; i++ ))
    do 
        logfile="/home/ubuntu/logs/twins-${now}_${i}.log"
        cargo xtest -p consensus twins_test_safety_attack_generator -- --nocapture >> "${logfile}" 2>&1
    done

# Generate testcases.
elif [ $CONFIG == 2 ]; then
    cargo xtest -p consensus twins_test_safety_attack_generator -- --nocapture

else
    echo "Wrong config parameter."
fi
