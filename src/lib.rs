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
/// pub fn do_with_fork(value: u64) -> u64 {
///     // Spawn a child process with a copy-on-write copy of memory
///     fork_map(|| {
///         // Do some obnoxious operation with `value`
///         // * Maybe it leaks memory
///         // * Maybe it uses static resources unsafely and
///         //   prevents multi-threaded operation
///         // * Maybe you couldn't figure out how to
///         //   send your data to a thread
///         Ok(value * 10)
///     }).unwrap()
///     // Execution continues after the child process has exited
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
///         fork_map(|| {
///             // Do your ugly operations here
///             Ok(item * 1234)
///         }).expect("fork_map succeeded")
///     }).collect::<Vec<_>>();
///
///     // Use results here
/// }
/// ```
pub fn fork_map<F, R>(func: F) -> anyhow::Result<R>
    where
        F: Fn() -> anyhow::Result<R>,
        R: Serialize + for<'a> Deserialize<'a>,
{
    // SAFETY: Probably not LOL, didn't crash on my box, use at your own risk, etc.

    // Pipe for sending the result from child to parent
    let mut pipe: [libc::c_int; 2] = [0; 2];
    unsafe {
        libc::pipe(pipe.as_mut_ptr());
    }

    // Here we go
    let pid = unsafe { libc::fork() };
    if pid == 0 {
        // Child
        unsafe { libc::close(pipe[0]) };
        let result = func().map_err(|e| serde_error::Error::new(&*e));
        let ser = serde_json::to_string(&result).unwrap_or("".to_string());
        unsafe { libc::write(pipe[1], ser.as_ptr() as *const libc::c_void, ser.len()) };
        unsafe { libc::close(pipe[1]) };
        unsafe { libc::exit(0) };
    }

    // Parent
    unsafe { libc::close(pipe[1]) };

    // Read result from pipe
    let mut des = vec![];
    let des = loop {
        const BUF_SIZE: usize = 0x1000;
        let mut buf: [u8; BUF_SIZE] = [0; BUF_SIZE];
        let count = unsafe { libc::read(pipe[0], buf.as_mut_ptr() as *mut libc::c_void, BUF_SIZE) };
        if count < 0 {
            break Err(anyhow!("io error: {}", unsafe { *libc::__error() }));
        }
        des.extend_from_slice(&buf[0..(count as usize)]);
        // EOF signalled by less than the max bytes
        if (count as usize) < BUF_SIZE {
            break Ok(des);
        }
    };

    let mut status = 0;
    unsafe { libc::waitpid(pid, &mut status, 0) };

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
