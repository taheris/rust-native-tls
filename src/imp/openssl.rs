extern crate openssl;

use std::io;
use std::fmt;
use std::error;
use self::openssl::pkcs12;
use self::openssl::error::ErrorStack;
use self::openssl::ssl::{self, SslMethod, SslConnectorBuilder, SslConnector, SslAcceptorBuilder,
                         SslAcceptor, MidHandshakeSslStream, SslOption, SslContextBuilder,
                         ShutdownResult};

use Protocol;

fn supported_protocols(protocols: &[Protocol], ctx: &mut SslContextBuilder) {
    // This constant is only defined on OpenSSL 1.0.2 and above, so manually do it.
    let ssl_op_no_ssl_mask = ssl::SSL_OP_NO_SSLV2 | ssl::SSL_OP_NO_SSLV3 |
        ssl::SSL_OP_NO_TLSV1 | ssl::SSL_OP_NO_TLSV1_2 | ssl::SSL_OP_NO_TLSV1_2;

    let mut options = ctx.options();
    ctx.clear_options(SslOption::all());
    options |= ssl_op_no_ssl_mask;
    for protocol in protocols {
        let op = match *protocol {
            Protocol::Sslv3 => ssl::SSL_OP_NO_SSLV3,
            Protocol::Tlsv10 => ssl::SSL_OP_NO_TLSV1,
            Protocol::Tlsv11 => ssl::SSL_OP_NO_TLSV1_1,
            Protocol::Tlsv12 => ssl::SSL_OP_NO_TLSV1_2,
            Protocol::__NonExhaustive => unreachable!(),
        };
        options &= !op;
    }
    ctx.set_options(options);
}

pub struct Error(ssl::Error);

impl error::Error for Error {
    fn description(&self) -> &str {
        error::Error::description(&self.0)
    }

    fn cause(&self) -> Option<&error::Error> {
        error::Error::cause(&self.0)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(&self.0, fmt)
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&self.0, fmt)
    }
}

impl From<ssl::Error> for Error {
    fn from(err: ssl::Error) -> Error {
        Error(err)
    }
}

impl From<ErrorStack> for Error {
    fn from(err: ErrorStack) -> Error {
        ssl::Error::Ssl(err).into()
    }
}

pub struct Pkcs12(pkcs12::ParsedPkcs12);

impl Pkcs12 {
    pub fn from_der(buf: &[u8], pass: &str) -> Result<Pkcs12, Error> {
        let pkcs12 = try!(pkcs12::Pkcs12::from_der(buf));
        let parsed = try!(pkcs12.parse(pass));
        Ok(Pkcs12(parsed))
    }
}

pub struct MidHandshakeTlsStream<S>(MidHandshakeSslStream<S>);

impl<S> fmt::Debug for MidHandshakeTlsStream<S>
    where S: fmt::Debug
{
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&self.0, fmt)
    }
}

impl<S> MidHandshakeTlsStream<S> {
    pub fn get_ref(&self) -> &S {
        self.0.get_ref()
    }

    pub fn get_mut(&mut self) -> &mut S {
        self.0.get_mut()
    }
}

impl<S> MidHandshakeTlsStream<S>
    where S: io::Read + io::Write
{
    pub fn handshake(self) -> Result<TlsStream<S>, HandshakeError<S>> {
        match self.0.handshake() {
            Ok(s) => Ok(TlsStream(s)),
            Err(e) => Err(e.into()),
        }
    }
}

pub enum HandshakeError<S> {
    Failure(Error),
    Interrupted(MidHandshakeTlsStream<S>),
}

impl<S> From<ssl::HandshakeError<S>> for HandshakeError<S> {
    fn from(e: ssl::HandshakeError<S>) -> HandshakeError<S> {
        match e {
            ssl::HandshakeError::SetupFailure(e) => {
                HandshakeError::Failure(Error(ssl::Error::Ssl(e)))
            }
            ssl::HandshakeError::Failure(e) => HandshakeError::Failure(Error(e.into_error())),
            ssl::HandshakeError::Interrupted(s) => {
                HandshakeError::Interrupted(MidHandshakeTlsStream(s))
            }
        }
    }
}

impl<S> From<ErrorStack> for HandshakeError<S> {
    fn from(e: ErrorStack) -> HandshakeError<S> {
        HandshakeError::Failure(e.into())
    }
}

pub struct TlsConnectorBuilder(SslConnectorBuilder);

impl TlsConnectorBuilder {
    pub fn identity(&mut self, pkcs12: Pkcs12) -> Result<(), Error> {
        let ctx = self.0.builder_mut();
        // FIXME clear chain certs to clean up if called multiple times
        try!(ctx.set_certificate(&pkcs12.0.cert));
        try!(ctx.set_private_key(&pkcs12.0.pkey));
        try!(ctx.check_private_key());
        for cert in pkcs12.0.chain {
            try!(ctx.add_extra_chain_cert(cert));
        }
        Ok(())
    }

    pub fn supported_protocols(&mut self, protocols: &[Protocol]) -> Result<(), Error> {
        supported_protocols(protocols, self.0.builder_mut());
        Ok(())
    }

    pub fn build(self) -> Result<TlsConnector, Error> {
        Ok(TlsConnector(self.0.build()))
    }
}

pub struct TlsConnector(SslConnector);

impl TlsConnector {
    pub fn builder() -> Result<TlsConnectorBuilder, Error> {
        let builder = try!(SslConnectorBuilder::new(SslMethod::tls()));
        Ok(TlsConnectorBuilder(builder))
    }

    pub fn connect<S>(&self, domain: &str, stream: S) -> Result<TlsStream<S>, HandshakeError<S>>
        where S: io::Read + io::Write
    {
        let s = try!(self.0.connect(domain, stream));
        Ok(TlsStream(s))
    }
}

/// OpenSSL-specific extensions to `TlsConnectorBuilder`.
pub trait TlsConnectorBuilderExt {
    /// Returns a shared reference to the inner `SslConnectorBuilder`.
    fn builder(&self) -> &SslConnectorBuilder;

    /// Returns a mutable reference to the inner `SslConnectorBuilder`.
    fn builder_mut(&mut self) -> &mut SslConnectorBuilder;
}

impl TlsConnectorBuilderExt for ::TlsConnectorBuilder {
    fn builder(&self) -> &SslConnectorBuilder {
        &(self.0).0
    }

    fn builder_mut(&mut self) -> &mut SslConnectorBuilder {
        &mut (self.0).0
    }
}

pub struct TlsAcceptorBuilder(SslAcceptorBuilder);

impl TlsAcceptorBuilder {
    pub fn supported_protocols(&mut self, protocols: &[Protocol]) -> Result<(), Error> {
        supported_protocols(protocols, self.0.builder_mut());
        Ok(())
    }

    pub fn build(self) -> Result<TlsAcceptor, Error> {
        Ok(TlsAcceptor(self.0.build()))
    }
}

pub struct TlsAcceptor(SslAcceptor);

impl TlsAcceptor {
    pub fn builder(pkcs12: Pkcs12) -> Result<TlsAcceptorBuilder, Error> {
        let builder = try!(SslAcceptorBuilder::mozilla_intermediate(
            SslMethod::tls(), &pkcs12.0.pkey, &pkcs12.0.cert, &pkcs12.0.chain));
        Ok(TlsAcceptorBuilder(builder))
    }

    pub fn accept<S>(&self, stream: S) -> Result<TlsStream<S>, HandshakeError<S>>
        where S: io::Read + io::Write
    {
        let s = try!(self.0.accept(stream));
        Ok(TlsStream(s))
    }
}

/// OpenSSL-specific extensions to `TlsAcceptorBuilder`.
pub trait TlsAcceptorBuilderExt {
    /// Returns a shared reference to the inner `SslAcceptorBuilder`.
    fn builder(&self) -> &SslAcceptorBuilder;

    /// Returns a mutable reference to the inner `SslAcceptorBuilder`.
    fn builder_mut(&mut self) -> &mut SslAcceptorBuilder;
}

impl TlsAcceptorBuilderExt for ::TlsAcceptorBuilder {
    fn builder(&self) -> &SslAcceptorBuilder {
        &(self.0).0
    }

    fn builder_mut(&mut self) -> &mut SslAcceptorBuilder {
        &mut (self.0).0
    }
}

pub struct TlsStream<S>(ssl::SslStream<S>);

impl<S: fmt::Debug> fmt::Debug for TlsStream<S> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&self.0, fmt)
    }
}

impl<S: io::Read + io::Write> TlsStream<S> {
    pub fn buffered_read_size(&self) -> Result<usize, Error> {
        Ok(self.0.ssl().pending())
    }

    pub fn shutdown(&mut self) -> io::Result<()> {
        loop {
            match self.0.shutdown() {
                Ok(ShutdownResult::Sent) => {},
                Ok(ShutdownResult::Received) => break,
                Err(ssl::Error::ZeroReturn) => break,
                Err(ssl::Error::Stream(e)) => return Err(e),
                Err(ssl::Error::WantRead(e)) => return Err(e),
                Err(ssl::Error::WantWrite(e)) => return Err(e),
                Err(e) => return Err(io::Error::new(io::ErrorKind::Other, e)),
            }
        }

        Ok(())
    }

    pub fn get_ref(&self) -> &S {
        self.0.get_ref()
    }

    pub fn get_mut(&mut self) -> &mut S {
        self.0.get_mut()
    }
}

impl<S: io::Read + io::Write> io::Read for TlsStream<S> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl<S: io::Read + io::Write> io::Write for TlsStream<S> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

/// OpenSSL-specific extensions to `TlsStream`.
pub trait TlsStreamExt<S> {
    /// Returns a shared reference to the OpenSSL `SslStream`.
    fn raw_stream(&self) -> &ssl::SslStream<S>;

    /// Returns a mutable reference to the OpenSSL `SslStream`.
    fn raw_stream_mut(&mut self) -> &mut ssl::SslStream<S>;
}

impl<S> TlsStreamExt<S> for ::TlsStream<S> {
    fn raw_stream(&self) -> &ssl::SslStream<S> {
        &(self.0).0
    }

    fn raw_stream_mut(&mut self) -> &mut ssl::SslStream<S> {
        &mut (self.0).0
    }
}
