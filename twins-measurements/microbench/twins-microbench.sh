#!/bin/bash

N=1

if [ "$1" != "" ]; then
	N=$1
fi

cd /Users/asonnino/Git/libra-twins/libra/consensus
echo "" > ~/Desktop/twins-microbench.log

for (( i=1; i<=$N; i++ ))
do 
	caffeinate -d -i -m -s -u time \
		cargo xtest -p consensus twins_test_safety_attack_generator -- --nocapture \
		1>> ~/Desktop/twins-microbench.log
done