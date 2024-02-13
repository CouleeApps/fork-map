fork_map
----

A Rust library for running operations in a child process spawned by `fork()`. Embrace fearful concurrency: fearful that your worker task will mess up your memory space.

## Example

```rust
use fork_map::fork_map;

pub fn do_with_fork(value: u64) -> u64 {
    // Spawn a child process with a copy-on-write copy of memory
    fork_map(|| {
        // Do some obnoxious operation with `value`
        // * Maybe it leaks memory
        // * Maybe it uses static resources unsafely and 
        //   prevents multi-threaded operation
        // * Maybe you couldn't figure out how to
        //   send your data to a thread
        Ok(value * 10)
    }).unwrap()
    // Execution continues after the child process has exited
}
```

## Motivation
Some operations work best if run in their own process. Whether they impose single-threaded restrictions, they consume untold resources when left running, or you just want to abuse copy-on-write memory to eliminate startup time, sometimes you really just want to `fork` and `map`. My main uses for this crate have been trying to embed libClang, which unsafely uses static memory because it assumes it is running single-threaded, and running operations that leak memory.

## Implementation and Support
`fork_map` is written using `libc::fork` and as such, will only work properly on *nix based systems that support `fork` (sorry Windows users!). Since the child process inherits the parent's memory space (as copy-on-write), there are no constraints on the input value or the operation. The result value is serialized using `serde_json` and sent over a `libc` file handle via some incredibly C-inspired unsafe io code. The parent process reads the data from the file and waits for the child to exit before returning.

## Use with `rayon`
It is generally expected that you will want to use this crate in conjunction with something like `rayon` since the call to `fork_map` blocks the thread of execution until the child process returns. In combination, you can have `rayon` coordinate a pool of worker threads that each spawn and control child processes with minimal boilerplate. A lot of my use cases end up looking something like this:

```rust
use fork_map::fork_map;
use rayon::prelude::*;

pub fn main() {
    let my_big_list = [ /* ... */ ];
    
    // Create a worker pool with rayon's into_par_iter
    let results = my_big_list.into_par_iter().map(|item| {
        // Have each worker spawn a child process for the
        // operations we don't want polluting the parent's memory
        fork_map(|| {
            // Do your ugly operations here
            Ok(item * 1234)
        }).expect("fork_map succeeded")
    }).collect::<Vec<_>>();

    // Use results here
}
```

If you have a lot of small tasks that you can run on a child process, you can use rayon's `chunks()` function and eliminate much of the overhead from calling `fork()` a lot (which can be significant):

```rust
use fork_map::fork_map;
use rayon::prelude::*;

pub fn main() {
    let my_big_list = [ /* ... */ ];
    
    // Use rayon's chunks() to give each forked process more
    // work to handle, if you have a lot of small tasks
    let results = my_big_list
        .into_par_iter()
        .chunks(512)
        .map(|items| {
            // Now each child process does 512 items at once
            fork_map(|| {
                let mut results = vec![];
                // Maybe this operation is only mildly heinous
                // and we can do 512 of them before the child
                // process needs to be restarted.
                for item in items {
                    results.push(item * 1234);
                }
                Ok(results)
            }).expect("fork_map succeeded")
        })
        .collect::<Vec<_>>();
}
```
