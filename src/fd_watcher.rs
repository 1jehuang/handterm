use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use winit::event_loop::EventLoopProxy;

pub fn spawn_fd_watcher<E: Clone + Send + 'static>(
    thread_name: &str,
    primary_fd: i32,
    secondary_fd: i32,
    proxy: EventLoopProxy<E>,
    event: E,
    stop: Arc<AtomicBool>,
) {
    let thread_name = thread_name.to_string();
    std::thread::Builder::new()
        .name(thread_name)
        .spawn(move || fd_watcher_thread(primary_fd, secondary_fd, proxy, event, stop))
        .expect("failed to spawn fd watcher thread");
}

fn fd_watcher_thread<E: Clone + Send + 'static>(
    primary_fd: i32,
    secondary_fd: i32,
    proxy: EventLoopProxy<E>,
    event: E,
    stop: Arc<AtomicBool>,
) {
    use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
    use std::os::fd::BorrowedFd;

    let mut fds = Vec::with_capacity(2);
    fds.push(PollFd::new(
        unsafe { BorrowedFd::borrow_raw(primary_fd) },
        PollFlags::POLLIN | PollFlags::POLLHUP,
    ));
    if secondary_fd >= 0 {
        fds.push(PollFd::new(
            unsafe { BorrowedFd::borrow_raw(secondary_fd) },
            PollFlags::POLLIN,
        ));
    }

    while !stop.load(Ordering::Relaxed) {
        match poll(&mut fds, PollTimeout::from(500u16)) {
            Ok(0) => continue,
            Ok(_) => {
                let has_data = fds[0]
                    .revents()
                    .is_some_and(|r| r.intersects(PollFlags::POLLIN));
                let secondary_data = fds.len() > 1
                    && fds[1]
                        .revents()
                        .is_some_and(|r| r.intersects(PollFlags::POLLIN));
                if has_data || secondary_data {
                    let _ = proxy.send_event(event.clone());
                }
                if fds[0]
                    .revents()
                    .is_some_and(|revents| revents.contains(PollFlags::POLLHUP))
                {
                    let _ = proxy.send_event(event.clone());
                    break;
                }
            }
            Err(nix::errno::Errno::EINTR) => continue,
            Err(_) => break,
        }
    }
}
