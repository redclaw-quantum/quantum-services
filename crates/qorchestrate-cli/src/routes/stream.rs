use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::sse::{Event, KeepAlive, Sse},
};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use uuid::Uuid;

use qorchestrate_executor::SseBroadcaster;

use crate::server::AppState;

pub async fn handle_stream(
    Path(id): Path<Uuid>,
    State(state): State<AppState>,
) -> Result<
    Sse<impl futures::Stream<Item = Result<Event, std::convert::Infallible>>>,
    StatusCode,
> {
    let rx = state
        .event_channels
        .read()
        .await
        .get(&id)
        .map(|tx| tx.subscribe())
        .ok_or(StatusCode::NOT_FOUND)?;

    let stream = BroadcastStream::new(rx).filter_map(|result| {
        result.ok().map(|event| {
            let name = SseBroadcaster::event_name(&event);
            let data = serde_json::to_string(&event).unwrap_or_default();
            Ok::<Event, std::convert::Infallible>(Event::default().event(name).data(data))
        })
    });

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}
