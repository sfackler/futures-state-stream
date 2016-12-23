extern crate futures;

use futures::{Async, Poll, Future, Stream};
use std::mem;

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

    #[inline]
    fn map_state<F, B>(self, f: F) -> MapState<Self, F>
        where Self: Sized,
              F: FnOnce(Self::State) -> B
    {
        MapState {
            stream: self,
            f: Some(f),
        }
    }

    #[inline]
    fn collect(self) -> Collect<Self>
        where Self: Sized
    {
        Collect {
            stream: self,
            items: vec![],
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

pub trait FutureExt: Future {
    #[inline]
    fn flatten_state_stream(self) -> FlattenStateStream<Self>
        where Self: Sized,
              Self::Item: StateStream<Error = Self::Error>
    {
        FlattenStateStream(FlattenStateStreamState::Future(self))
    }
}

impl<F: ?Sized> FutureExt for F
    where F: Future
{}

#[inline]
pub fn stream<S>(stream: S) -> FromStream<S>
    where S: Stream
{
    FromStream(stream)
}

#[inline]
pub fn unfold<T, F, Fut, It, St>(init: T, f: F) -> Unfold<T, F, Fut>
     where F: FnMut(T) -> Fut,
           Fut: futures::IntoFuture<Item = StreamEvent<(It, T), St>>
{
    Unfold {
        state: UnfoldState::Ready(init),
        f: f,
    }
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

pub struct MapState<S, F> {
    stream: S,
    f: Option<F>,
}

impl<S, F, B> StateStream for MapState<S, F>
    where S: StateStream,
          F: FnOnce(S::State) -> B
{
    type Item = S::Item;
    type State = B;
    type Error = S::Error;

    #[inline]
    fn poll(&mut self) -> Poll<StreamEvent<S::Item, B>, S::Error> {
        self.stream.poll().map(|a| {
            match a {
                Async::Ready(StreamEvent::Next(i)) => Async::Ready(StreamEvent::Next(i)),
                Async::Ready(StreamEvent::Done(s)) => {
                    let f = self.f.take().expect("polled MapState after completion");
                    Async::Ready(StreamEvent::Done(f(s)))
                }
                Async::NotReady => Async::NotReady,
            }
        })
    }
}

pub struct Collect<S>
    where S: StateStream
{
    stream: S,
    items: Vec<S::Item>,
}

impl<S> Future for Collect<S>
    where S: StateStream
{
    type Item = (Vec<S::Item>, S::State);
    type Error = S::Error;

    #[inline]
    fn poll(&mut self) -> Poll<(Vec<S::Item>, S::State), S::Error> {
        loop {
            match self.stream.poll() {
                Ok(Async::Ready(StreamEvent::Next(i))) => self.items.push(i),
                Ok(Async::Ready(StreamEvent::Done(s))) => {
                    let items = mem::replace(&mut self.items, vec![]);
                    return Ok(Async::Ready((items, s)))
                }
                Ok(Async::NotReady) => return Ok(Async::NotReady),
                Err(e) => return Err(e),
            }
        }
    }
}

pub struct Unfold<T, F, Fut>
    where Fut: futures::IntoFuture
{
    state: UnfoldState<T, Fut::Future>,
    f: F,
}

enum UnfoldState<T, F>
{
    Empty,
    Ready(T),
    Processing(F),
}

impl<T, F, Fut, It, St> StateStream for Unfold<T, F, Fut>
    where F: FnMut(T) -> Fut,
          Fut: futures::IntoFuture<Item = StreamEvent<(It, T), St>>
{
    type Item = It;
    type State = St;
    type Error = Fut::Error;

    #[inline]
    fn poll(&mut self) -> Poll<StreamEvent<It, St>, Fut::Error> {
        loop {
            match mem::replace(&mut self.state, UnfoldState::Empty) {
                UnfoldState::Empty => panic!("polled an Unfold after completion"),
                UnfoldState::Ready(state) => {
                    self.state = UnfoldState::Processing((self.f)(state).into_future())
                }
                UnfoldState::Processing(mut fut) => {
                    match try!(fut.poll()) {
                        Async::Ready(StreamEvent::Next((i, state))) => {
                            self.state = UnfoldState::Ready(state);
                            return Ok(Async::Ready(StreamEvent::Next(i)));
                        }
                        Async::Ready(StreamEvent::Done(s)) => {
                            return Ok(Async::Ready(StreamEvent::Done(s)));
                        }
                        Async::NotReady => {
                            self.state = UnfoldState::Processing(fut);
                            return Ok(Async::NotReady);
                        }
                    }
                }
            }
        }
    }
}

enum FlattenStateStreamState<F>
    where F: Future
{
    Future(F),
    Stream(F::Item),
}

pub struct FlattenStateStream<F>(FlattenStateStreamState<F>)
    where F: Future;

impl<F> StateStream for FlattenStateStream<F>
    where F: Future,
          F::Item: StateStream<Error = F::Error>
{
    type Item = <F::Item as StateStream>::Item;
    type State = <F::Item as StateStream>::State;
    type Error = F::Error;

    #[inline]
    fn poll(&mut self) -> Poll<StreamEvent<Self::Item, Self::State>, Self::Error> {
        loop {
            self.0 = match self.0 {
                FlattenStateStreamState::Future(ref mut f) => {
                    match f.poll() {
                        Ok(Async::NotReady) => return Ok(Async::NotReady),
                        Ok(Async::Ready(stream)) => FlattenStateStreamState::Stream(stream),
                        Err(e) => return Err(e),
                    }
                }
                FlattenStateStreamState::Stream(ref mut s) => return s.poll(),
            };
        }
    }
}