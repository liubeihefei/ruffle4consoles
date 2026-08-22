use async_channel::{Receiver, Sender};
use encoding_rs::Encoding;
use indexmap::IndexMap;
use percent_encoding::percent_decode_str;
use ruffle_core::backend::navigator::{
    ErrorResponse, NavigationMethod, NavigatorBackend, NullExecutor, NullSpawner, OwnedFuture,
    Request, SuccessResponse, async_return, create_specific_fetch_error,
};
use ruffle_core::loader::Error;
use ruffle_core::socket::{ConnectionState, SocketAction, SocketHandle};
use std::borrow::Cow;
use std::fmt;
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;
use url::{ParseError, Url};

/// Navigator for Vita's `ux0:` filesystem, which `url::Url::to_file_path`
/// does not support on this target.
pub struct VitaNavigatorBackend {
    spawner: NullSpawner,
    root: PathBuf,
}

impl VitaNavigatorBackend {
    pub fn new(root: impl Into<PathBuf>, executor: &NullExecutor) -> Self {
        Self {
            spawner: executor.spawner(),
            root: root.into(),
        }
    }
}

impl NavigatorBackend for VitaNavigatorBackend {
    fn navigate_to_url(
        &self,
        _url: &str,
        _target: &str,
        _vars_method: Option<(NavigationMethod, IndexMap<String, String>)>,
    ) {
    }

    fn fetch(&self, request: Request) -> OwnedFuture<Box<dyn SuccessResponse>, ErrorResponse> {
        let requested_url = request.url().to_owned();
        let resolved_url = match self.resolve_url(&requested_url) {
            Ok(url) => url,
            Err(error) => {
                println!("VITA_NAV_REJECT url={requested_url} reason={error}");
                return async_return(Err(create_specific_fetch_error(
                    "Invalid Vita local URL",
                    &requested_url,
                    error,
                )));
            }
        };

        if !matches!(resolved_url.scheme(), "file" | "ux0") {
            println!(
                "VITA_NAV_REJECT url={} reason=non-local scheme {}",
                resolved_url,
                resolved_url.scheme()
            );
            return async_return(Err(create_specific_fetch_error(
                "VitaNavigatorBackend can't fetch non-local URL",
                resolved_url.as_str(),
                "",
            )));
        }

        let relative_path = match relative_local_path(&resolved_url) {
            Ok(path) => path,
            Err(error) => {
                println!("VITA_NAV_REJECT url={resolved_url} reason={error}");
                return async_return(Err(create_specific_fetch_error(
                    "Invalid Vita local path",
                    resolved_url.as_str(),
                    error,
                )));
            }
        };
        let path = self.root.join(relative_path);
        let response_url = resolved_url.to_string();

        println!(
            "VITA_NAV_FETCH request={} resolved={} path={}",
            requested_url,
            response_url,
            path.display()
        );

        Box::pin(async move {
            let expected_length = match std::fs::metadata(&path) {
                Ok(metadata) if metadata.is_file() => metadata.len(),
                Ok(_) => {
                    println!("VITA_NAV_OPEN_ERR path={} reason=not a file", path.display());
                    return Err(create_specific_fetch_error(
                        "Vita local path is not a file",
                        &response_url,
                        path.display(),
                    ));
                }
                Err(error) => {
                    println!("VITA_NAV_OPEN_ERR path={} reason={error}", path.display());
                    return Err(create_specific_fetch_error(
                        "Unable to open Vita local file",
                        &response_url,
                        error,
                    ));
                }
            };

            println!(
                "VITA_NAV_OPEN_OK path={} bytes={expected_length}",
                path.display()
            );
            let response: Box<dyn SuccessResponse> = Box::new(VitaLocalResponse {
                url: response_url,
                path,
                open_file: None,
                expected_length,
            });
            Ok(response)
        })
    }

    fn resolve_url(&self, url: &str) -> Result<Url, ParseError> {
        match Url::parse(url) {
            Ok(parsed) => {
                if matches!(parsed.scheme(), "file" | "ux0")
                    && raw_path_is_unsafe(url)
                {
                    return Err(ParseError::RelativeUrlWithoutBase);
                }
                Ok(self.pre_process_url(parsed))
            }
            Err(ParseError::RelativeUrlWithoutBase) => {
                if raw_path_is_unsafe(url) {
                    return Err(ParseError::RelativeUrlWithoutBase);
                }
                let base = Url::parse("file:///").expect("static Vita base URL must be valid");
                base.join(url).map(|url| self.pre_process_url(url))
            }
            Err(error) => Err(error),
        }
    }

    fn spawn_future(&mut self, future: OwnedFuture<(), Error>) {
        self.spawner.spawn_local(future);
    }

    fn pre_process_url(&self, url: Url) -> Url {
        url
    }

    fn connect_socket(
        &mut self,
        _host: String,
        _port: u16,
        _timeout: Duration,
        handle: SocketHandle,
        _receiver: Receiver<Vec<u8>>,
        sender: Sender<SocketAction>,
    ) {
        sender
            .try_send(SocketAction::Connect(handle, ConnectionState::Failed))
            .expect("working channel send");
    }
}

struct VitaLocalResponse {
    url: String,
    path: PathBuf,
    open_file: Option<File>,
    expected_length: u64,
}

impl SuccessResponse for VitaLocalResponse {
    fn url(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.url)
    }

    fn body(self: Box<Self>) -> OwnedFuture<Vec<u8>, Error> {
        Box::pin(async move {
            std::fs::read(&self.path).map_err(|error| {
                Error::FetchError(format!("{}: {error}", self.path.display()))
            })
        })
    }

    fn text_encoding(&self) -> Option<&'static Encoding> {
        None
    }

    fn status(&self) -> u16 {
        0
    }

    fn redirected(&self) -> bool {
        false
    }

    fn next_chunk(&mut self) -> OwnedFuture<Option<Vec<u8>>, Error> {
        if self.open_file.is_none() {
            match File::open(&self.path) {
                Ok(file) => self.open_file = Some(file),
                Err(error) => {
                    let message = format!("{}: {error}", self.path.display());
                    return Box::pin(async move { Err(Error::FetchError(message)) });
                }
            }
        }

        let mut buffer = vec![0; 4096];
        let result = self.open_file.as_mut().expect("file opened above").read(&mut buffer);
        Box::pin(async move {
            match result {
                Ok(0) => Ok(None),
                Ok(count) => {
                    buffer.truncate(count);
                    Ok(Some(buffer))
                }
                Err(error) => Err(Error::FetchError(error.to_string())),
            }
        })
    }

    fn expected_length(&self) -> Result<Option<u64>, Error> {
        Ok(Some(self.expected_length))
    }
}

#[derive(Debug, Eq, PartialEq)]
enum LocalPathError {
    RemoteFileHost,
    InvalidEncoding,
    InvalidSegment,
    OutsideRoot,
}

impl fmt::Display for LocalPathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::RemoteFileHost => "file URL has a remote host",
            Self::InvalidEncoding => "path contains invalid UTF-8 escaping",
            Self::InvalidSegment => "path contains an unsafe segment",
            Self::OutsideRoot => "path is outside ux0:data/ruffle",
        };
        formatter.write_str(message)
    }
}

fn relative_local_path(url: &Url) -> Result<PathBuf, LocalPathError> {
    if url.scheme() == "file" && url.host_str().is_some() {
        return Err(LocalPathError::RemoteFileHost);
    }

    let mut segments = decode_segments(url.path())?;
    match url.scheme() {
        "file" => {
            if segments
                .first()
                .is_some_and(|segment| segment.eq_ignore_ascii_case("ux0:"))
            {
                segments.remove(0);
                strip_vita_root(&mut segments)?;
            }
        }
        "ux0" => strip_vita_root(&mut segments)?,
        _ => return Err(LocalPathError::OutsideRoot),
    }

    let mut path = PathBuf::new();
    for segment in segments {
        path.push(segment);
    }
    Ok(path)
}

fn decode_segments(path: &str) -> Result<Vec<String>, LocalPathError> {
    path.split('/')
        .filter(|segment| !segment.is_empty())
        .map(|segment| {
            let decoded = percent_decode_str(segment)
                .decode_utf8()
                .map_err(|_| LocalPathError::InvalidEncoding)?;
            if decoded == "."
                || decoded == ".."
                || contains_forbidden_separator(&decoded)
                || (decoded.contains(':') && !decoded.eq_ignore_ascii_case("ux0:"))
            {
                return Err(LocalPathError::InvalidSegment);
            }
            Ok(decoded.into_owned())
        })
        .collect()
}

fn strip_vita_root(segments: &mut Vec<String>) -> Result<(), LocalPathError> {
    if segments.len() < 2
        || !segments[0].eq_ignore_ascii_case("data")
        || !segments[1].eq_ignore_ascii_case("ruffle")
    {
        return Err(LocalPathError::OutsideRoot);
    }
    segments.drain(0..2);
    Ok(())
}

fn raw_path_is_unsafe(url: &str) -> bool {
    let without_fragment = url
        .split_once('#')
        .map_or(url, |(path, _)| path);
    let path = without_fragment
        .split_once('?')
        .map_or(without_fragment, |(path, _)| path);

    if path.contains('\\') {
        return true;
    }

    path.split('/').any(|segment| {
        percent_decode_str(segment).decode_utf8().map_or(true, |decoded| {
            decoded == ".." || contains_forbidden_separator(&decoded)
        })
    })
}

fn contains_forbidden_separator(value: &str) -> bool {
    value
        .chars()
        .any(|character| matches!(character, '/' | '\\' | '\0'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(url: &str) -> PathBuf {
        relative_local_path(&Url::parse(url).unwrap()).unwrap()
    }

    #[test]
    fn maps_supported_local_url_forms() {
        let expected = PathBuf::from("assets").join("OtherMat1.swf");
        assert_eq!(path("file:///assets/OtherMat1.swf?cache=1#part"), expected);
        assert_eq!(path("ux0:/data/ruffle/assets/OtherMat1.swf"), expected);
        assert_eq!(
            path("file:///ux0:/data/ruffle/assets/OtherMat1.swf"),
            expected
        );
    }

    #[test]
    fn decodes_safe_percent_escapes() {
        assert_eq!(
            path("file:///assets/My%20Movie.swf"),
            PathBuf::from("assets").join("My Movie.swf")
        );
    }

    #[test]
    fn rejects_paths_outside_the_vita_root() {
        assert_eq!(
            relative_local_path(&Url::parse("ux0:/app/secret.bin").unwrap()),
            Err(LocalPathError::OutsideRoot)
        );
        assert_eq!(
            relative_local_path(&Url::parse("file://server/assets/a.swf").unwrap()),
            Err(LocalPathError::RemoteFileHost)
        );
    }

    #[test]
    fn detects_traversal_and_separator_tricks_before_url_normalization() {
        assert!(raw_path_is_unsafe("./assets/../secret.bin"));
        assert!(raw_path_is_unsafe("file:///assets/%2e%2e/secret.bin"));
        assert!(raw_path_is_unsafe("file:///assets/%2Fsecret.bin"));
        assert!(raw_path_is_unsafe("file:///assets/%5csecret.bin"));
    }
}
