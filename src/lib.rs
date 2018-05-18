extern crate either;

use std::boxed::Box;
use std::convert::From;
use std::error::Error;
use std::fmt;
use std::marker;
use std::sync::mpsc::{Sender, Receiver};
use std::sync::mpsc;

use either::Either;


/// The error types used.

#[derive(Debug)]
pub struct Cancel;

#[derive(Debug)]
pub struct SendError<T>(mpsc::SendError<T>);

#[derive(Debug)]
pub struct ReceiveError(mpsc::RecvError);

impl fmt::Display for Cancel {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        "canceling session".fmt(f)
    }
}

impl<T> fmt::Display for SendError<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl fmt::Display for ReceiveError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl Error for Cancel {
    fn description(&self) -> &str {
        "canceling session"
    }
}

impl<T: fmt::Debug + marker::Send> Error for SendError<T> {
    fn cause(&self) -> Option<&Error> {
        Some(&self.0)
    }
}

impl Error for ReceiveError {
    fn cause(&self) -> Option<&Error> {
        Some(&self.0)
    }
}

impl<T> From<mpsc::SendError<T>> for SendError<T> {
    fn from(error: mpsc::SendError<T>) -> Self {
        SendError(error)
    }
}

impl From<mpsc::RecvError> for ReceiveError {
    fn from(error: mpsc::RecvError) -> Self {
        ReceiveError(error)
    }
}


/// The session types supported.

#[derive(Debug)]
pub struct End;

#[derive(Debug)]
pub struct Send<T, S: Session> {
    channel: Sender<(T, S::Dual)>,
}

#[derive(Debug)]
pub struct Receive<T, S: Session> {
    channel: Receiver<(T, S)>,
}

pub trait Session: marker::Sized + marker::Send {
    type Dual: Session<Dual=Self>;

    fn new() -> (Self, Self::Dual);
}

impl Session for End {
    type Dual = End;

    fn new() -> (Self, Self::Dual) {
        return (End, End);
    }
}

impl<T: marker::Send, S: Session> Session for Send<T, S> {
    type Dual = Receive<T, S::Dual>;

    fn new() -> (Self, Self::Dual) {
        let (sender, receiver) = mpsc::channel::<(T, S::Dual)>();
        return (Send { channel: sender }, Receive { channel: receiver });
    }
}

impl<T: marker::Send, S: Session> Session for Receive<T, S> {
    type Dual = Send<T, S::Dual>;

    fn new() -> (Self, Self::Dual) {
        let (there, here) = Self::Dual::new();
        return (here, there);
    }
}


/// The communication primitives.

pub fn send<'a, T: marker::Send + 'a, S: Session + 'a>(x: T, s: Send<T, S>) -> Result<S, Box<Error + 'a>> {
    let (here, there) = S::new();
    s.channel.send((x, there))?;
    Ok(here)
}

pub fn receive<'a, T: marker::Send, S: Session>(s: Receive<T, S>) -> Result<(T, S), Box<Error + 'a>> {
    let (v, s) = s.channel.recv()?;
    Ok((v, s))
}

pub fn close(s: End) -> Result<(), Box<Error>> {
    let End = s;
    Ok(())
}

pub fn cancel<T>() -> Result<T, Box<Error>> {
    Err(Box::new(Cancel))
}

#[macro_export]
macro_rules! fork {
    (move | $session_name:ident : $session_type:ty | $forked_process:block ) => {{
        let ($session_name, here) = <$session_type as $crate::Session>::new();
        ::std::thread::spawn(move || {
            (move || -> Result<_, Box<Error>> {
                $forked_process
            })().unwrap();
        });
        here
    }};
}

pub type Offer<S1, S2> = Receive<Either<S1, S2>, End>;
pub type Select<S1, S2> = Send<Either<<S1 as Session>::Dual, <S2 as Session>::Dual>, End>;

pub fn offer_either<'a, S1: Session, S2: Session, F, G, R>(s: Offer<S1, S2>, f: F, g: G) -> Result<R, Box<Error + 'a>>
where
    F: FnOnce(S1) -> Result<R, Box<Error + 'a>>,
    G: FnOnce(S2) -> Result<R, Box<Error + 'a>>,
{
    let (e, End) = receive(s)?;
    e.either(f, g)
}

// SendError<(Either<S1::Dual, S2::Dual>, End)>
pub fn select_left<'a, S1: Session + 'a, S2: Session + 'a>(s: Select<S1, S2>) -> Result<S1, Box<Error + 'a>> {
    let (here, there) = S1::new();
    let End = send(Either::Left(there), s)?;
    Ok(here)
}

// SendError<(Either<S1::Dual, S2::Dual>, End)>
pub fn select_right<'a, S1: Session + 'a, S2: Session + 'a>(s: Select<S1, S2>) -> Result<S2, Box<Error + 'a>> {
    let (here, there) = S2::new();
    let End = send(Either::Right(there), s)?;
    Ok(here)
}

#[macro_export]
macro_rules! offer {
    ($session:expr, { $($pat:pat => $result:expr,)* }) => {
        (move || -> Result<_, _> {
            let (l, End) = receive($session)?;
            match l {
                $(
                    $pat => $result,
                )*
            }
        })();
    };
}

#[macro_export]
macro_rules! select {
    ($label:path, $session:expr) => {
        (move || -> Result<_, Box<Error>> {
            let (here, there) = <_ as Session>::new();
            let End = send($label(there), $session)?;
            Ok(here)
        })();
    };
}

#[cfg(test)]
#[allow(dead_code)]
mod tests {
    extern crate rand;

    use std::marker;
    use super::*;
    use self::rand::{Rng, thread_rng};

    // Test sending a ping across threads.

    #[test]
    fn ping_works() {
        assert!(|| -> Result<(), Box<Error>> {

            let s = fork!(move |s: Send<(), End>| {
                let s = send((), s)?;
                close(s)
            });
            let ((), s) = receive(s)?;
            close(s)

        }().is_ok());
    }

    // Test a simple calculator server, implemented using binary choice.

    type NegServer<N> = Receive<N, Send<N, End>>;
    type NegClient<N> = <NegServer<N> as Session>::Dual;

    type AddServer<N> = Receive<N, Receive<N, Send<N, End>>>;
    type AddClient<N> = <AddServer<N> as Session>::Dual;

    type SimpleCalcServer<N> = Offer<NegServer<N>, AddServer<N>>;
    type SimpleCalcClient<N> = <SimpleCalcServer<N> as Session>::Dual;

    #[test]
    fn simple_calc_works() {
        assert!(|| -> Result<(), Box<Error>> {

            // Pick some random numbers.
            let mut rng = thread_rng();
            let x: i32 = rng.gen();
            let y: i32 = rng.gen();

            // Create a calculator server and send it the numbers.
            let s = fork!(move |s: SimpleCalcServer<i32>| {
                offer_either(s,
                      |s: NegServer<i32>| {
                          let (x, s) = receive(s)?;
                          let s = send(-x, s)?;
                          close(s)
                      },
                      |s: AddServer<i32>| {
                          let (x, s) = receive(s)?;
                          let (y, s) = receive(s)?;
                          let s = send(x.wrapping_add(y), s)?;
                          close(s)
                      })

            });

            let s = select_right::<NegClient<i32>, _>(s)?;
            let s = send(x, s)?;
            let s = send(y, s)?;
            let (z, End) = receive(s)?;

            // Check if the server worked.
            assert_eq!(x.wrapping_add(y), z);
            Ok(())

        }().is_ok());
    }

    // Test a nice calculator server, implemented using variant types.

    enum Op<N: marker::Send> {
        Neg(NegServer<N>),
        Add(AddServer<N>),
    }
    type NiceCalcServer<N> = Receive<Op<N>, End>;
    type NiceCalcClient<N> = <NiceCalcServer<N> as Session>::Dual;

    #[test]
    fn nice_server_works() {
        assert!(|| -> Result<(), Box<Error>> {

            // Pick some random numbers.
            let mut rng = thread_rng();
            let x: i32 = rng.gen();
            let y: i32 = rng.gen();

            let s = fork!(move |s: NiceCalcServer<i32>| {
                offer!(s, {
                    Op::Neg(s) => {
                        let (x, s) = receive(s)?;
                        let s = send(-x, s)?;
                        close(s)
                    },
                    Op::Add(s) => {
                        let (x, s) = receive(s)?;
                        let (y, s) = receive(s)?;
                        let s = send(x.wrapping_add(y), s)?;
                        close(s)
                    },
                })
            });

            let s = select!(Op::Add, s)?;
            let s = send(x, s)?;
            let s = send(y, s)?;
            let (z, s) = receive(s)?;
            close(s)?;

            // Check if the server worked correctly.
            assert_eq!(x.wrapping_add(y), z);

            Ok(())

        }().is_ok());
    }

}

// */
// */
// */
// */
// */
