use anyhow::anyhow;
use serde::{Deserialize, Serialize};

/// Forks, and runs function F in a child process.
/// Waits for the child to terminate and returns the result of F.
///
/// # Example
///
/// ```
/// use fork_map::fork_map;
///
/// // Result type needs to implement serde::Serialize and serde::Deserialize
/// pub fn do_with_fork(value: u64) -> u64 {
///     // Spawn a child process with a copy-on-write copy of memory
///     unsafe {
///         fork_map(|| {
///             // Do some obnoxious operation with `value`
///             // * Maybe it leaks memory
///             // * Maybe it uses static resources unsafely and
///             //   prevents multi-threaded operation
///             // * Maybe you couldn't figure out how to
///             //   send your data to a thread
///             Ok(value * 10)
///         }).unwrap()
///         // Execution continues after the child process has exited
///     }
/// }
/// ```
///
/// Often used in conjunction with `rayon` since `fork_map` will block until the child terminates,
/// so you can construct a worker pool where each job is executed in a child process:
///
/// # Example
/// ```
/// use fork_map::fork_map;
/// use rayon::prelude::*;
///
/// pub fn main() {
///     let my_big_list = [ /* ... */ ];
///
///     // Create a worker pool with rayon's into_par_iter
///     let results = my_big_list.into_par_iter().map(|item| {
///         // Have each worker spawn a child process for the
///         // operations we don't want polluting the parent's memory
///         unsafe {
///             fork_map(|| {
///                 // Do your ugly operations here
///                 Ok(item * 1234)
///             }).expect("fork_map succeeded")
///         }
///     }).collect::<Vec<_>>();
///
///     // Use results here
/// }
/// ```
///
/// # Safety
///
/// Due to the nature of `fork()`, this function is very unsound and likely violates most of Rust's
/// guarantees about lifetimes, considering all of your memory gets duplicated into a second
/// process, even though it calls `exit(0)` after your closure is executed. Any threads other than
/// the one calling `fork_map` will not be present in the new process, so threaded lifetime
/// guarantees are also violated. Don't even think about using async executors with this.
pub unsafe fn fork_map<F, R>(func: F) -> anyhow::Result<R>
    where
        F: Fn() -> anyhow::Result<R>,
        R: Serialize + for<'a> Deserialize<'a>,
{
    // Pipe for sending the result from child to parent
    let mut pipe: [libc::c_int; 2] = [0; 2];
    libc::pipe(pipe.as_mut_ptr());

    // Here we go
    let pid = libc::fork();
    if pid == 0 {
        // Child
        libc::close(pipe[0]);
        let result = func().map_err(|e| serde_error::Error::new(&*e));
        let ser = serde_json::to_string(&result).unwrap_or("".to_string());
        libc::write(pipe[1], ser.as_ptr() as *const libc::c_void, ser.len());
        libc::close(pipe[1]);
        libc::exit(0);
    }

    // Parent
    libc::close(pipe[1]);

    // Read result from pipe
    let mut des = vec![];
    let des = loop {
        const BUF_SIZE: usize = 0x1000;
        let mut buf: [u8; BUF_SIZE] = [0; BUF_SIZE];
        let count = libc::read(pipe[0], buf.as_mut_ptr() as *mut libc::c_void, BUF_SIZE);
        if count < 0 {
            break Err(anyhow!("io error: {}", *libc::__error()));
        }
        des.extend_from_slice(&buf[0..(count as usize)]);
        // EOF signalled by less than the max bytes
        if (count as usize) < BUF_SIZE {
            break Ok(des);
        }
    };

    let mut status = 0;
    libc::waitpid(pid, &mut status, 0);

    if status != 0 {
        return Err(anyhow!("Process returned non-zero status code {}", status));
    }

    des.and_then(|des| {
        serde_json::from_slice::<Result<R, serde_error::Error>>(des.as_slice())
            .map_err(|e| anyhow!("{}", e))
            .and_then(|se| match se {
                Ok(i) => Ok(i),
                Err(e) => Err(anyhow::Error::from(e)),
            })
    })
}
