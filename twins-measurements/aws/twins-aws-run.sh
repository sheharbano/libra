#!/bin/bash

N=1

if [ "$1" != "" ]; then
	N=$1
fi

mkdir -p logs
now=$(date +"%Y-%m-%d:%H:%M:%S")

cd libra/consensus
source $HOME/.cargo/env

for (( i=1; i<=$N; i++ ))
do 
	logfile="/home/ubuntu/logs/twins-${now}_${i}.log"
	cargo xtest -p consensus twins_test_safety_attack_generator -- --nocapture >> "${logfile}" 2>&1
done
