# Simple yaml task runner (sytr)

> a simple reference implementation to show the use of [abstract pipeline runner](https://github.com/nikita-skobov/abstract-pipeline-runner).

This project is a binary application that takes the path to a single yaml file, and then runs the file according to series or parallel steps.

## Example

Consider a yaml file such as:

```yml
hey_node:
    run: |
        echo I AM ECHOING ?{{ 1 }} MYENV = $MYENV;
        sleep 1;
    env:
        MYENV: ?{{ 2 }}

series:
    - echo this happens first
    - parallel:
          - hey_node hey1 thisismyenv?
          - hey_node hey2 thisisnmyenv?
          - hey_node hey3 ymeeenv?
```

This project looks for one of 3 keywords at the root level:

- series
- parallel
- run

If it finds one of those at the root level, it will use that
as the root node. In this case, we have a series node at the root.

A series node gets ran by the `abstract-pipeline-runner` in order, and each task waits for the previous task before running.

A parallel node runs all of its tasks at once. Here, we have a parallel
node nested in a series node. The parallel node consists of references to globally available nodes, in this case it references `hey_node`.

In a sense, your nodes can "call" each other. So the parallel node calls 3 instances of `hey_node`, and each time it passes it different arguments.

Inside `hey_node` it uses the first argument to echo something, and the second argument as an environment variable (that it also echoes).

If you run this, it will output "this happens first", and then a second later it will output all of the `hey_node` outputs at the same time because they are ran in parallel.

## Installation

```
cargo build --release
```

## Running

```
./target/release/sytr <path-to-yaml-file> [...context args]
```

## Please note

This is just a reference implementation. It's minimal, and buggy, and not meant to be a replacement for anything serious.

That being said, I wrote this to be able to run tests for [mgt](https://github.com/nikita-skobov/monorepo-git-tools) **much** faster.

My original test runner was a big bash script that ran everything sequentially and was quite ugly. I replaced it with the following yaml file that accomplishes the same thing but faster:

```yml
# mgt test runner looks something similar to this

run_e2e_test:
  run: bats ?{{ 1 }}
  env:
    PROGRAM_PATH: ?{{ program_path }}

series:
  - name: build
    series:
      - run: cargo build --release 2>&1
        name: cargo build
      - run: realpath ./target/release/mgt
        capture_stdout: program_path
        name: cargo build cleanup

  - parallel:
    - cargo test
    - run_e2e_test test/general
    - run_e2e_test test/splitout
    - run_e2e_test test/splitin
    - run_e2e_test test/splitinas
    - run_e2e_test test/topbase
    - run_e2e_test test/splitoutas
    - run_e2e_test test/check
```

Sequentially, my tests ran for about 24 seconds.

With this simple yaml task runner, it took about 7 seconds.

