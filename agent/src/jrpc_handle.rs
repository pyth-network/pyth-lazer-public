use crate::config::Config;
use crate::lazer_publisher::LazerPublisher;
use crate::metadata::{
    DROPPED_IDENTIFIER_UPDATES, IDENTIFIER_MAP_LOADED, IdentifierMap, fetch_metadata,
};
use crate::websocket_utils::{handle_websocket_error, send_json, send_text};
use arc_swap::ArcSwap;
use futures::{AsyncRead, AsyncWrite};
use futures_util::io::{BufReader, BufWriter};
use hyper_util::rt::TokioIo;
use pyth_lazer_protocol::jrpc::{
    FeedUpdateParams, GetMetadataParams, JrpcCall, JrpcError, JrpcErrorResponse, JrpcId,
    JrpcResponse, JrpcSuccessResponse, JsonRpcVersion, PythLazerAgentJrpcV1, ResolvedFeedUpdate,
    SymbolMetadata,
};
use soketto::Sender;
use soketto::handshake::http::Server;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio::{pin, select};
use tokio_util::compat::TokioAsyncReadCompatExt;
use tracing::{debug, error, instrument};

pub struct JrpcConnectionContext {
    pub identifier_map: Arc<ArcSwap<IdentifierMap>>,
}

#[instrument(
    skip(server, request, lazer_publisher, context),
    fields(component = "jrpc_ws")
)]
pub async fn handle_jrpc(
    config: Config,
    server: Server,
    request: hyper::Request<hyper::body::Incoming>,
    context: JrpcConnectionContext,
    lazer_publisher: LazerPublisher,
) {
    if let Err(err) = try_handle_jrpc(config, server, request, context, lazer_publisher).await {
        handle_websocket_error(err);
    }
}

#[instrument(
    skip(server, request, lazer_publisher, context),
    fields(component = "jrpc_ws")
)]
async fn try_handle_jrpc(
    config: Config,
    server: Server,
    request: hyper::Request<hyper::body::Incoming>,
    context: JrpcConnectionContext,
    lazer_publisher: LazerPublisher,
) -> anyhow::Result<()> {
    let stream = hyper::upgrade::on(request).await?;
    let io = TokioIo::new(stream);
    let stream = BufReader::new(BufWriter::new(io.compat()));
    let (mut ws_sender, mut ws_receiver) = server.into_builder(stream).finish();

    let mut receive_buf = Vec::new();

    loop {
        receive_buf.clear();
        {
            // soketto is not cancel-safe, so we need to store the future and poll it
            // in the inner loop.
            let receive = async { ws_receiver.receive(&mut receive_buf).await };
            pin!(receive);
            #[allow(clippy::never_loop, reason = "false positive")] // false positive
            loop {
                select! {
                    _result = &mut receive => {
                        break
                    }
                }
            }
        }

        match handle_jrpc_inner(
            &config,
            &context,
            &mut ws_sender,
            &mut receive_buf,
            &lazer_publisher,
        )
        .await
        {
            Ok(_) => {}
            Err(err) => {
                debug!("Error handling JRPC request: {}", err);
                send_text(
                    &mut ws_sender,
                    serde_json::to_string::<JrpcResponse<()>>(&JrpcResponse::Error(
                        JrpcErrorResponse {
                            jsonrpc: JsonRpcVersion::V2,
                            error: JrpcError::InternalError(err.to_string()).into(),
                            id: JrpcId::Null,
                        },
                    ))?
                    .as_str(),
                )
                .await?;
            }
        }
    }
}

async fn handle_jrpc_inner<T: AsyncRead + AsyncWrite + Unpin>(
    config: &Config,
    context: &JrpcConnectionContext,
    sender: &mut Sender<T>,
    receive_buf: &mut Vec<u8>,
    lazer_publisher: &LazerPublisher,
) -> anyhow::Result<()> {
    match serde_json::from_slice::<PythLazerAgentJrpcV1>(receive_buf.as_slice()) {
        Ok(jrpc_request) => match jrpc_request.params {
            JrpcCall::PushUpdate(request_params) => {
                push_updates(
                    sender,
                    context,
                    vec![request_params],
                    jrpc_request.id,
                    lazer_publisher,
                )
                .await
            }
            JrpcCall::PushUpdates(request_params_batch) => {
                push_updates(
                    sender,
                    context,
                    request_params_batch,
                    jrpc_request.id,
                    lazer_publisher,
                )
                .await
            }
            JrpcCall::GetMetadata(request_params) => match jrpc_request.id {
                JrpcId::Null => {
                    send_json(
                        sender,
                        &JrpcErrorResponse {
                            jsonrpc: JsonRpcVersion::V2,
                            error: JrpcError::ParseError(
                                "The request to method 'get_metadata' requires an 'id'".to_string(),
                            )
                            .into(),
                            id: JrpcId::Null,
                        },
                    )
                    .await
                }
                _ => handle_get_metadata(sender, config, request_params, jrpc_request.id).await,
            },
        },
        Err(err) => {
            debug!("Error parsing JRPC request: {}", err);
            send_json(
                sender,
                &JrpcErrorResponse {
                    jsonrpc: JsonRpcVersion::V2,
                    error: JrpcError::ParseError(err.to_string()).into(),
                    id: JrpcId::Null,
                },
            )
            .await
        }
    }
}

async fn push_updates<T: AsyncRead + AsyncWrite + Unpin>(
    sender: &mut Sender<T>,
    context: &JrpcConnectionContext,
    batch: Vec<FeedUpdateParams>,
    request_id: JrpcId,
    lazer_publisher: &LazerPublisher,
) -> anyhow::Result<()> {
    let resolved = {
        let map = context.identifier_map.load();
        batch
            .into_iter()
            .enumerate()
            .map(|(index, params)| {
                resolve_feed_ids(params, &map).map_err(|err| match err {
                    JrpcError::InvalidParams(message) => {
                        JrpcError::InvalidParams(format!("update {index}: {message}"))
                    }
                    other => other,
                })
            })
            .collect::<Result<Vec<_>, _>>()
    };
    let resolved = match resolved {
        Ok(resolved) => resolved,
        Err(err) => {
            return send_json(
                sender,
                &JrpcErrorResponse {
                    jsonrpc: JsonRpcVersion::V2,
                    error: err.into(),
                    id: request_id,
                },
            )
            .await;
        }
    };
    for update in resolved.into_iter().flatten() {
        if let Err(err) = lazer_publisher
            .push_feed_update(update.clone().into())
            .await
        {
            return send_update_failure_response(sender, update.into(), request_id, err).await;
        }
    }
    send_update_success_response(sender, request_id).await
}

fn resolve_feed_ids(
    params: FeedUpdateParams,
    map: &IdentifierMap,
) -> Result<Vec<ResolvedFeedUpdate>, JrpcError> {
    if let Some(feed_id) = params.feed_id {
        return Ok(vec![params.resolve(feed_id)]);
    }
    let Some(identifier) = &params.identifier else {
        return Err(JrpcError::InvalidParams(
            "feed_id or identifier is required".to_string(),
        ));
    };
    if !IDENTIFIER_MAP_LOADED.load(Ordering::Relaxed) {
        return Err(JrpcError::InternalError(
            "identifier map not loaded yet, retry shortly".to_string(),
        ));
    }
    let feed_ids = map
        .get(&identifier.normalized())
        .map(Vec::as_slice)
        .unwrap_or_default();
    if feed_ids.is_empty() {
        DROPPED_IDENTIFIER_UPDATES.fetch_add(1, Ordering::Relaxed);
        debug!(?identifier, "no feed for identifier, dropping update");
    }
    Ok(feed_ids
        .iter()
        .map(|&feed_id| params.clone().resolve(feed_id))
        .collect())
}

fn filter_symbols(
    symbols: Vec<SymbolMetadata>,
    get_metadata_params: GetMetadataParams,
) -> Vec<SymbolMetadata> {
    let names = &get_metadata_params.names.clone();
    let asset_types = &get_metadata_params.asset_types.clone();

    let res: Vec<SymbolMetadata> = symbols
        .into_iter()
        .filter(|symbol| {
            if let Some(names) = names {
                if !names.contains(&symbol.name) {
                    return false;
                }
            }

            if let Some(asset_types) = asset_types {
                if !asset_types.contains(&symbol.asset_type) {
                    return false;
                }
            }

            true
        })
        .collect();

    res
}

async fn send_update_success_response<T: AsyncRead + AsyncWrite + Unpin>(
    sender: &mut Sender<T>,
    request_id: JrpcId,
) -> anyhow::Result<()> {
    match request_id {
        JrpcId::Null => Ok(()),
        _ => {
            send_json(
                sender,
                &JrpcSuccessResponse::<String> {
                    jsonrpc: JsonRpcVersion::V2,
                    result: "success".to_string(),
                    id: request_id,
                },
            )
            .await
        }
    }
}

async fn send_update_failure_response<T: AsyncRead + AsyncWrite + Unpin>(
    sender: &mut Sender<T>,
    request_params: FeedUpdateParams,
    request_id: JrpcId,
    err: anyhow::Error,
) -> anyhow::Result<()> {
    debug!("error while sending updates: {:?}", err);
    send_json(
        sender,
        &JrpcErrorResponse {
            jsonrpc: JsonRpcVersion::V2,
            error: JrpcError::SendUpdateError(request_params).into(),
            id: request_id,
        },
    )
    .await
}

async fn handle_get_metadata<T: AsyncRead + AsyncWrite + Unpin>(
    sender: &mut Sender<T>,
    config: &Config,
    request_params: GetMetadataParams,
    request_id: JrpcId,
) -> anyhow::Result<()> {
    match fetch_metadata(&config.history_service_url).await {
        Ok(symbols) => {
            let symbols = filter_symbols(symbols.clone(), request_params);

            send_json(
                sender,
                &JrpcSuccessResponse::<Vec<SymbolMetadata>> {
                    jsonrpc: JsonRpcVersion::V2,
                    result: symbols,
                    id: request_id,
                },
            )
            .await
        }
        Err(err) => {
            error!("error while retrieving metadata: {:?}", err);
            send_json(
                sender,
                &JrpcErrorResponse {
                    jsonrpc: JsonRpcVersion::V2,
                    error: JrpcError::InternalError(err.to_string()).into(),
                    id: request_id,
                },
            )
            .await
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::metadata::test_symbol;
    use pyth_lazer_protocol::PriceFeedId;
    use pyth_lazer_protocol::jrpc::{Identifier, UpdateParams};
    use pyth_lazer_protocol::time::TimestampUs;

    use super::*;

    fn gen_test_symbol(name: String, asset_type: String) -> SymbolMetadata {
        test_symbol(1, &name, &asset_type)
    }

    fn params(feed_id: Option<PriceFeedId>, identifier: Option<Identifier>) -> FeedUpdateParams {
        FeedUpdateParams {
            feed_id,
            identifier,
            source_timestamp: TimestampUs::from_micros(1),
            update: UpdateParams::PriceUpdate {
                price: None,
                best_bid_price: None,
                best_ask_price: None,
                trading_status: None,
                market_session: None,
            },
        }
    }

    #[test]
    fn test_resolve_feed_ids() {
        let map = IdentifierMap::from([
            (
                Identifier::cusip("912797NQ4", "%"),
                vec![PriceFeedId(1), PriceFeedId(2)],
            ),
            (Identifier::cusip("912797NQ4", "USD"), vec![PriceFeedId(3)]),
        ]);
        let by_cusip = |quote: &str| params(None, Some(Identifier::cusip(" 912797nq4 ", quote)));

        let by_id = params(Some(PriceFeedId(9)), None);
        assert_eq!(
            resolve_feed_ids(by_id.clone(), &map).unwrap(),
            vec![by_id.resolve(PriceFeedId(9))]
        );

        assert!(matches!(
            resolve_feed_ids(by_cusip("%"), &map),
            Err(JrpcError::InternalError(_))
        ));
        IDENTIFIER_MAP_LOADED.store(true, Ordering::Relaxed);

        let yields = resolve_feed_ids(by_cusip("%"), &map).unwrap();
        assert_eq!(
            yields.iter().map(|p| p.feed_id).collect::<Vec<_>>(),
            vec![PriceFeedId(1), PriceFeedId(2)]
        );
        assert!(
            yields
                .iter()
                .all(|p| p.identifier == Some(Identifier::cusip(" 912797nq4 ", "%")))
        );
        assert_eq!(
            resolve_feed_ids(by_cusip("usd"), &map).unwrap()[0].feed_id,
            PriceFeedId(3)
        );
        assert!(resolve_feed_ids(by_cusip("GBP"), &map).unwrap().is_empty());
        assert!(
            resolve_feed_ids(params(None, Some(Identifier::cusip("nope", "%"))), &map)
                .unwrap()
                .is_empty()
        );

        assert!(matches!(
            resolve_feed_ids(params(None, None), &map),
            Err(JrpcError::InvalidParams(_))
        ));
    }

    #[test]
    fn test_filter_symbols() {
        let symbol1 = gen_test_symbol("BTC".to_string(), "crypto".to_string());
        let symbol2 = gen_test_symbol("XMR".to_string(), "crypto".to_string());
        let symbol3 = gen_test_symbol("BTCUSDT".to_string(), "funding-rate".to_string());
        let symbols = vec![symbol1.clone(), symbol2.clone(), symbol3.clone()];

        // just a name filter
        assert_eq!(
            filter_symbols(
                symbols.clone(),
                GetMetadataParams {
                    names: Some(vec!["XMR".to_string()]),
                    asset_types: None,
                },
            ),
            vec![symbol2.clone()]
        );

        // just an asset type filter
        assert_eq!(
            filter_symbols(
                symbols.clone(),
                GetMetadataParams {
                    names: None,
                    asset_types: Some(vec!["crypto".to_string()]),
                },
            ),
            vec![symbol1.clone(), symbol2.clone()]
        );

        // name and asset type
        assert_eq!(
            filter_symbols(
                symbols.clone(),
                GetMetadataParams {
                    names: Some(vec!["BTC".to_string()]),
                    asset_types: Some(vec!["crypto".to_string()]),
                },
            ),
            vec![symbol1.clone()]
        );
    }
}
