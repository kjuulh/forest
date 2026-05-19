use std::{
    pin::Pin,
    task::{Context, Poll},
};

use tokio::time::Instant;
use tower::{Layer, Service};

#[derive(Debug, Clone, Default)]
pub struct LogMiddlewareLayer {}

impl<S> Layer<S> for LogMiddlewareLayer {
    type Service = LogMiddelware<S>;

    fn layer(&self, service: S) -> Self::Service {
        LogMiddelware { inner: service }
    }
}

#[derive(Debug, Clone)]
pub struct LogMiddelware<S> {
    inner: S,
}

type BoxFuture<'a, T> = Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

impl<S, ReqBody, ResBody> Service<http::Request<ReqBody>> for LogMiddelware<S>
where
    S: Service<http::Request<ReqBody>, Response = http::Response<ResBody>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    ReqBody: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: http::Request<ReqBody>) -> Self::Future {
        let method = req.method().clone();
        let uri = req.uri().clone();

        // See: https://docs.rs/tower/latest/tower/trait.Service.html#be-careful-when-cloning-inner-services
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);

        Box::pin(async move {
            let start = Instant::now();
            let response = inner.call(req).await?;
            let elapsed = start.elapsed();
            let status = response.status();
            let path = uri.path();

            tracing::debug!(method = %method,
                 path,
                 status=%status,
                 elapsed_ms = elapsed.as_millis(),
                 elapsed_mu = elapsed.as_micros(),
                 "request");

            Ok(response)
        })
    }
}
