extern crate futures;

use futures::{Async, Poll, Future, Stream};

pub type BoxStateStream<T, S, E> = Box<StateStream<Item = T, State = S, Error = E> + Send>;

pub enum StreamEvent<I, S> {
    Next(I),
    Done(S),
}

pub trait StateStream {
    type Item;
    type State;
    type Error;

    fn poll(&mut self) -> Poll<StreamEvent<Self::Item, Self::State>, Self::Error>;

    #[inline]
    fn boxed(self) -> BoxStateStream<Self::Item, Self::State, Self::Error>
        where Self: Sized + Send + 'static
    {
        Box::new(self)
    }

    #[inline]
    fn into_future(self) -> IntoFuture<Self>
        where Self: Sized
    {
        IntoFuture(Some(self))
    }

    #[inline]
    fn into_stream(self) -> IntoStream<Self>
        where Self: Sized
    {
        IntoStream(self)
    }

    #[inline]
    fn map<F, B>(self, f: F) -> Map<Self, F>
        where Self: Sized,
              F: FnMut(Self::Item) -> B
    {
        Map {
            stream: self,
            f: f,
        }
    }

    #[inline]
    fn map_err<F, B>(self, f: F) -> MapErr<Self, F>
        where Self: Sized,
              F: FnMut(Self::Error) -> B
    {
        MapErr {
            stream: self,
            f: f,
        }
    }
}

impl<S: ?Sized> StateStream for Box<S>
    where S: StateStream
{
    type Item = S::Item;
    type State = S::State;
    type Error = S::Error;

    #[inline]
    fn poll(&mut self) -> Poll<StreamEvent<S::Item, S::State>, S::Error> {
        S::poll(self)
    }
}

pub fn stream<S>(stream: S) -> FromStream<S>
    where S: Stream
{
    FromStream(stream)
}

pub struct FromStream<S>(S);

impl<S> StateStream for FromStream<S>
    where S: Stream
{
    type Item = S::Item;
    type State = ();
    type Error = S::Error;

    #[inline]
    fn poll(&mut self) -> Poll<StreamEvent<S::Item, ()>, S::Error> {
        self.0.poll().map(|a| {
            match a {
                Async::Ready(Some(i)) => Async::Ready(StreamEvent::Next(i)),
                Async::Ready(None) => Async::Ready(StreamEvent::Done(())),
                Async::NotReady => Async::NotReady,
            }
        })
    }
}

pub struct IntoFuture<S>(Option<S>);

impl<S> Future for IntoFuture<S>
    where S: StateStream
{
    type Item = (StreamEvent<S::Item, S::State>, S);
    type Error = (S::Error, S);

    #[inline]
    fn poll(&mut self) -> Poll<(StreamEvent<S::Item, S::State>, S), (S::Error, S)> {
        let item = match self.0.as_mut().expect("polling IntoFuture twice").poll() {
            Ok(Async::NotReady) => return Ok(Async::NotReady),
            Ok(Async::Ready(i)) => Ok(i),
            Err(e) => Err(e),
        };
        let stream = self.0.take().unwrap();
        match item {
            Ok(i) => Ok(Async::Ready((i, stream))),
            Err(e) => Err((e, stream)),
        }
    }
}

pub struct IntoStream<S>(S);

impl<S> Stream for IntoStream<S>
    where S: StateStream
{
    type Item = S::Item;
    type Error = S::Error;

    #[inline]
    fn poll(&mut self) -> Poll<Option<S::Item>, S::Error> {
        self.0.poll().map(|a| {
            match a {
                Async::Ready(StreamEvent::Next(i)) => Async::Ready(Some(i)),
                Async::Ready(StreamEvent::Done(_)) => Async::Ready(None),
                Async::NotReady => Async::NotReady,
            }
        })
    }
}

pub struct Map<S, F> {
    stream: S,
    f: F,
}

impl<S, F, B> StateStream for Map<S, F>
    where S: StateStream,
          F: FnMut(S::Item) -> B
{
    type Item = B;
    type State = S::State;
    type Error = S::Error;

    #[inline]
    fn poll(&mut self) -> Poll<StreamEvent<B, S::State>, S::Error> {
        self.stream.poll().map(|a| {
            match a {
                Async::Ready(StreamEvent::Next(i)) => Async::Ready(StreamEvent::Next((self.f)(i))),
                Async::Ready(StreamEvent::Done(s)) => Async::Ready(StreamEvent::Done(s)),
                Async::NotReady => Async::NotReady,
            }
        })
    }
}

pub struct MapErr<S, F> {
    stream: S,
    f: F,
}

impl<S, F, B> StateStream for MapErr<S, F>
    where S: StateStream,
          F: FnMut(S::Error) -> B
{
    type Item = S::Item;
    type State = S::State;
    type Error = B;

    #[inline]
    fn poll(&mut self) -> Poll<StreamEvent<S::Item, S::State>, B> {
        match self.stream.poll() {
            Ok(a) => Ok(a),
            Err(e) => Err((self.f)(e))
        }
    }
}
