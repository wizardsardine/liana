use coincube_gui::phone_signer::protocol::{local_v1, LocalEnvelope};
use prost::Message;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
// Transport-only fake. Real durable cross-application stores are exercised by
// the native driver. This helper must not be described as persistence coverage.
pub async fn complete<T: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut T,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    complete_with_fault(stream, None).await
}
pub async fn complete_with_fault<T: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut T,
    fault: Option<(usize, &str)>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use local_v1::{local_envelope::Payload, pairing_step::Phase};
    let mut id = None;
    for (boundary, (expected, reply)) in [
        (Phase::Accept, Phase::Prepared),
        (Phase::Commit, Phase::Committed),
        (Phase::Finish, Phase::Finished),
    ]
    .iter()
    .copied()
    .enumerate()
    {
        let length = stream.read_u32().await? as usize;
        if length > 16384 {
            return Err("oversized pairing frame".into());
        }
        let mut data = vec![0; length];
        stream.read_exact(&mut data).await?;
        let env = LocalEnvelope::decode(&*data)?;
        let Some(Payload::PairingStep(step)) = env.payload else {
            return Err("pairing refused".into());
        };
        if step.phase != expected as i32 || id.as_ref().is_some_and(|id| id != &step.transaction_id)
        {
            return Err("unexpected phase".into());
        }
        if let Some((stop, kind)) = fault {
            if boundary == stop {
                if kind == "timeout" {
                    tokio::time::sleep(std::time::Duration::from_secs(11)).await;
                }
                if kind == "unexpected" {
                    let bad = LocalEnvelope {
                        payload: Some(Payload::PairingStep(local_v1::PairingStep {
                            transaction_id: "wrong-id".into(),
                            phase: reply as i32,
                        })),
                    }
                    .encode_to_vec();
                    stream.write_u32(bad.len() as u32).await?;
                    stream.write_all(&bad).await?;
                    stream.flush().await?;
                }
                return Err("injected transport fault".into());
            }
        }
        id = Some(step.transaction_id.clone());
        let reply = LocalEnvelope {
            payload: Some(Payload::PairingStep(local_v1::PairingStep {
                transaction_id: step.transaction_id,
                phase: reply as i32,
            })),
        }
        .encode_to_vec();
        stream.write_u32(reply.len() as u32).await?;
        stream.write_all(&reply).await?;
        stream.flush().await?;
    }
    Ok(())
}
