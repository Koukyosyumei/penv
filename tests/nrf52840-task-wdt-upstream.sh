#!/bin/sh
# NRF52840.robot / Should Run Task Watchdog Subsystem: check two ordered cycles
# in the output of the original ELF from nrf52840-audio-upstream.sh.
set -eu
if [ "$#" -ne 1 ]; then
    echo "Usage: $0 TASK_WATCHDOG_UART_LOG" >&2
    exit 1
fi

awk '
    BEGIN {
        expected[1] = "Task watchdog sample application."
        expected[2] = "Main thread still alive..."
        expected[3] = "Control thread started."
        expected[4] = "Main thread still alive..."
        expected[5] = "Main thread still alive..."
        expected[6] = "Main thread still alive..."
        expected[7] = "Control thread getting stuck..."
        expected[8] = "Main thread still alive..."
        expected[9] = "Task watchdog channel 1 callback, thread: control"
        expected[10] = "Resetting device..."
        next_line = 1
    }
    index($0, expected[next_line]) {
        next_line++
        if (next_line == 11) {
            completed++
            next_line = 1
            if (completed == 2) exit
        }
        next
    }
    next_line > 1 && index($0, expected[1]) {
        printf "Missing line in reset cycle %d: %s\n", completed + 1, expected[next_line]
        failed = 1
        exit
    }
    END {
        if (failed) exit 1
        if (completed < 2) {
            printf "Missing line in reset cycle %d: %s\n", completed + 1, expected[next_line]
            exit 1
        }
        print "PASS two upstream task watchdog sequences"
    }
' "$1"
