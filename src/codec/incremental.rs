//! Input-budget enforcement around the shared whole-file state machine.

use enough::Stop;
use whereat::{At, at};
use zencodec::decode::{DecodeRowSink, IncrementalDecode, Input, Progress, Request, StepError};
use zencodec::{CodecError, ImageInfo, ResourceLimits};
use zenpixels::PixelDescriptor;

use super::decode_job::AvifDecodeJob;
use crate::Error;

pub(super) struct AvifIncremental {
    inner: zencodec::helpers::WholeFileIncremental<AvifDecodeJob>,
    max_input_bytes: Option<u64>,
    requests: Vec<Request>,
}

impl AvifIncremental {
    pub(super) fn new(
        job: AvifDecodeJob,
        preferred: &[PixelDescriptor],
        max_input_bytes: Option<u64>,
    ) -> Self {
        let mut this = Self {
            inner: zencodec::helpers::WholeFileIncremental::new(job, preferred),
            max_input_bytes,
            requests: Vec::with_capacity(1),
        };
        this.update_requests();
        this
    }

    fn update_requests(&mut self) {
        self.requests.clear();
        // One extra byte distinguishes an exactly-at-limit unknown-length
        // source from an oversized one, without requesting the whole source.
        let cap = self
            .max_input_bytes
            .map(|n| usize::try_from(n.saturating_add(1)).unwrap_or(usize::MAX));
        for &request in self.inner.requested() {
            self.requests.push(match (request, cap) {
                (Request::AtLeast { offset, min_len }, Some(cap)) => {
                    Request::at_least(offset, min_len.min(cap))
                }
                (other, _) => other,
            });
        }
    }
}

impl IncrementalDecode for AvifIncremental {
    type Error = At<CodecError>;

    fn step(
        &mut self,
        input: Input<'_>,
        sink: &mut dyn DecodeRowSink,
        stop: &dyn Stop,
    ) -> Result<Progress, StepError<Self::Error>> {
        stop.check()?;
        if let Some(limit) = self.max_input_bytes {
            let size = input
                .total_len
                .unwrap_or(input.bytes.len() as u64)
                .max(input.bytes.len() as u64);
            ResourceLimits::none()
                .with_max_input_bytes(limit)
                .check_input_size(size)
                .map_err(|e| {
                    StepError::Codec(CodecError::of(at!(Error::ResourceLimit(e.to_string()))))
                })?;
        }
        let progress = self.inner.step(input, sink, stop)?;
        self.update_requests();
        Ok(progress)
    }

    fn requested(&self) -> &[Request] {
        &self.requests
    }

    fn info(&self) -> Option<&ImageInfo> {
        self.inner.info()
    }

    fn output_descriptor(&self) -> Option<PixelDescriptor> {
        self.inner.output_descriptor()
    }

    fn access_pattern(&self) -> zencodec::source::AccessPattern {
        self.inner.access_pattern()
    }
}
