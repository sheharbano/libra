#!/bin/bash

mkdir -p logs

cd libra/consensus
source $HOME/.cargo/env

testcases=/home/ubuntu/logs/testcase*.bin
for file in $testcases
do 
    name="$(basename -- $file)"
    cp $file ./to_execute.bin
	logfile="/home/ubuntu/logs/${name}.log"
	cargo xtest -p consensus execute_testcases_from_file -- --nocapture >> "${logfile}" 2>&1
done
