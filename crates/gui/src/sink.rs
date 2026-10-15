//! The link's messages, carried to the UI thread. The link's threads send
//! into an unbounded futures channel; a task on GPUI's foreground executor
//! drains it (see `fleet`), so nothing on the UI thread ever blocks.

use ainet_model::{Msg, Sink};
use futures::channel::mpsc;

/// The sending end the link's threads hold.
#[derive(Clone)]
pub struct ChannelSink(mpsc::UnboundedSender<Msg>);

/// A sink and the receiver the UI drains.
pub fn channel() -> (ChannelSink, mpsc::UnboundedReceiver<Msg>) {
    let (tx, rx) = mpsc::unbounded();
    (ChannelSink(tx), rx)
}

impl Sink for ChannelSink {
    fn send(&self, msg: Msg) -> bool {
        self.0.unbounded_send(msg).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use futures::StreamExt;
    use futures::executor::block_on;

    use super::*;

    #[test]
    fn delivers_in_order_from_any_thread() {
        let (sink, mut rx) = channel();
        let other = sink.clone();
        std::thread::spawn(move || {
            assert!(other.send(Msg::Connected(true)));
            assert!(other.send(Msg::Flash("hi".into(), false)));
        })
        .join()
        .unwrap();
        assert!(matches!(block_on(rx.next()), Some(Msg::Connected(true))));
        assert!(matches!(block_on(rx.next()), Some(Msg::Flash(t, false)) if t == "hi"));
    }

    #[test]
    fn send_fails_once_the_ui_is_gone() {
        let (sink, rx) = channel();
        drop(rx);
        assert!(!sink.send(Msg::Connected(false)));
    }

    #[test]
    fn stream_ends_when_every_sender_is_dropped() {
        let (sink, mut rx) = channel();
        drop(sink);
        assert!(block_on(rx.next()).is_none());
    }
}
