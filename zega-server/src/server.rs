use crate::{routes, AppState};
use std::{
    io,
    net::{IpAddr, Ipv4Addr},
};
use tokio::net::TcpListener;

pub async fn serve(listener: TcpListener, state: AppState) -> io::Result<()> {
    if state.token_hash.is_none() && listener.local_addr()?.ip() != IpAddr::V4(Ipv4Addr::LOCALHOST)
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "a token is required when binding outside 127.0.0.1",
        ));
    }
    axum::serve(listener, routes::app(state)).await
}
