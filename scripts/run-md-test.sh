#!/bin/bash

function clear_environment() {
    clear_logs
    clear_docker
    free_ports
    clear_redis_file
}

function clear_logs() {
    rm -rf logs
}

function clear_docker() {
    docker stop $(docker ps -aq) 2>/dev/null
    docker rm $(docker ps -aq) 2>/dev/null
    docker network rm interledger 2>/dev/null
}

function free_ports() {
    # ports of redis
    for port in $(seq 6379 6385); do
        if lsof -Pi :${port} -sTCP:LISTEN -t >/dev/null ; then
            redis-cli -p ${port} shutdown
        fi
    done

    # ports of redis
    for port in $(seq 6379 6385); do
        if lsof -tPi :${port} >/dev/null ; then
            kill `lsof -tPi :${port}`
        fi
    done

    # ports of ganache + node + se
    for port in 8545 7770 8770 9770 3000 3001 3002 3003; do
        if lsof -tPi :${port} >/dev/null ; then
            kill `lsof -tPi :${port}`
        fi
    done
}

function clear_redis_file() {
    # redis file
    if [ -f dump.rdb ] ; then
        rm -f dump.rdb
    fi
}

# $1 = target directory
# $2 = docker mode in [01]
#
# test_example examples/eth-settlement 0
function test_example() {
    local target_directory=$1
    local docker_mode=$2
    local target_name=$(basename $target_directory)
    local result

    pushd $target_directory > /dev/null
    if [ $docker_mode -eq 1 ]; then
        printf "\e[33;1mTesting \"${target_name}\" on docker mode. [%d/%d]\e[m\n" "$((TESTING_INDEX + 1))" "${TESTS_TOTAL}"
        clear_environment; TEST_MODE=1 USE_DOCKER=1 $RUN_MD .
        result=$?
    else
        printf "\e[33;1mTesting \"${target_name}\" on non-docker mode. [%d/%d]\e[m\n" "$((TESTING_INDEX + 1))" "${TESTS_TOTAL}"
        clear_environment; TEST_MODE=1 $RUN_MD .
        result=$?
    fi
    mkdir -p ${LOG_DIR}/${target_name}
    mv logs ${LOG_DIR}/${target_name}/${docker_mode}
    popd > /dev/null
    return $result
}

# Adds all directories in `examples` directory as test targets.
function add_example_tests() {
    # This will add tests like "test_example examples/eth-settlement 0" which means an example test of
    # eth-settlement in non-docker mode.
    for directory in $(find $BASE_DIR/../examples/* -maxdepth 0 -type d); do
        TESTS+=("test_example ${directory} 0") # this cannot contain space in the directory path
        TESTS+=("test_example ${directory} 1")
    done
}

BASE_DIR=$(cd $(dirname $0); pwd)
RUN_MD=$BASE_DIR/run-md.sh
LOG_DIR=/tmp/run-md-test/logs

# set up example tests
TESTS=()
add_example_tests

# set up log dir for CircleCI
# the log files will be put as artifacts
rm -rf $LOG_DIR
mkdir -p $LOG_DIR

TESTS_TOTAL=${#TESTS[@]}
TESTS_FAILED=0
TESTING_INDEX=0
for test in "${TESTS[@]}"; do
    $test
    if [ $? -ne 0 ]; then
        TESTS_FAILED=$((TESTS_FAILED + 1))
    fi
    TESTING_INDEX=$((TESTING_INDEX + 1))
done
if [ $TESTS_FAILED -eq 0 ]; then
    printf "\e[32;1mAll tests passed! [%d/%d]\e[m\n" $((TESTS_TOTAL - TESTS_FAILED)) "${TESTS_TOTAL}"
    exit 0
else
    printf "\e[31;1mSome tests failed. [%d/%d]\e[m\n" $((TESTS_TOTAL - TESTS_FAILED)) "${TESTS_TOTAL}"
    exit 1
fi
