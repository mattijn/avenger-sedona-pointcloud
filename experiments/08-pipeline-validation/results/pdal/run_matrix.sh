#!/usr/bin/env bash
# Run every fixture through `pdal pipeline --validate` and a real execute.
# PDAL_TEST_DATA points at PDAL's own test/data (a source checkout, tag 2.10.2).
#   PDAL=pdal PDAL_TEST_DATA=~/src/pdal/test/data ./run_matrix.sh
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
PDAL=${PDAL:-pdal}
W=$(mktemp -d); OUT=$W/out; mkdir -p $OUT; R=$HERE/results; mkdir -p $R
for f in $HERE/pipelines/*.json; do
  n=$(basename $f .json)
  sed -e "s#@PDAL_TEST_DATA@#$PDAL_TEST_DATA#g" -e "s#@OUT@#$OUT#g" $f > $W/$n.json
  rm -f $OUT/*
  $PDAL pipeline --validate $W/$n.json > $R/$n.validate.txt 2>&1; echo "exit=$?" >> $R/$n.validate.txt
  $PDAL pipeline $W/$n.json > $R/$n.execute.txt 2>&1; echo "exit=$?" >> $R/$n.execute.txt
done
echo "results in $R"
