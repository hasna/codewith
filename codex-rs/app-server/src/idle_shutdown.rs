use tokio::sync::mpsc;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum TransportEventOrIdle<T> {
    Event(Option<T>),
    Idle,
}

pub(super) async fn receive_transport_event_or_idle<T>(
    receiver: &mut mpsc::Receiver<T>,
    idle_deadline: Option<tokio::time::Instant>,
) -> TransportEventOrIdle<T> {
    tokio::select! {
        biased;
        event = receiver.recv() => TransportEventOrIdle::Event(event),
        () = async {
            match idle_deadline {
                Some(deadline) if deadline <= tokio::time::Instant::now() => {}
                Some(deadline) => tokio::time::sleep_until(deadline).await,
                None => std::future::pending::<()>().await,
            }
        } => TransportEventOrIdle::Idle,
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::TransportEventOrIdle;
    use super::receive_transport_event_or_idle;

    #[tokio::test]
    async fn queued_connection_event_always_wins_at_exact_idle_deadline() {
        for connection_id in 0..64 {
            let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
            sender.send(connection_id).await.expect("queue event");

            assert_eq!(
                receive_transport_event_or_idle(&mut receiver, Some(tokio::time::Instant::now()),)
                    .await,
                TransportEventOrIdle::Event(Some(connection_id)),
            );
        }
    }
}
