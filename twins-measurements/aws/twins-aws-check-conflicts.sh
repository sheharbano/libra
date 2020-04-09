#!/bin/bash

mkdir -p logs # Contains log files.

files="/home/ubuntu/logs/*.log"
for file in $files
do 
    conflicts="$(cat $file | grep CONFLICT)"
    if [ -z "$conflicts" ] ; then
        echo "$file..........OK."
    else
        echo "Conflict detected!"
        echo $conflicts
    fi
done
