//! A version of the `futures` crate's `Stream` which returns a state value when it completes.
//!
//! This is useful in contexts where a value "becomes" a stream for a period of time, but then
//! switches back to its original type.
#![warn(missing_docs)]
#![doc(html_root_url="https://docs.rs/futures-state-stream/0.2")]

#[macro_use]
extern crate futures;

use futures::{Async, Poll, Future, Stream};
use std::mem;
use std::panic::AssertUnwindSafe;

/// An event from a `StateStream`.
pub enum StreamEvent<I, S> {
    /// The next item in the stream.
    Next(I),
    /// The state of the stream, returned when streaming has completed.
    Done(S),
}

/// A variant of `futures::Stream` which returns a state value when it completes.
pub trait StateStream {
    /// The items being streamed.
    type Item;
    /// The state returned when streaming completes.
    type State;
    /// The error returned.
    type Error;

    /// Similar to `Stream::poll`.
    ///
    /// The end of a stream is indicated by a `StreamEvent::Done` value. The result of calling
    /// `poll` after the end of the stream or an error has been reached is unspecified.
    fn poll(&mut self) -> Poll<StreamEvent<Self::Item, Self::State>, (Self::Error, Self::State)>;

    /// Returns a future which yields the next element of the stream.
    #[inline]
    fn into_future(self) -> IntoFuture<Self>
    where
        Self: Sized,
    {
        IntoFuture(Some(self))
    }

    /// Returns a normal `Stream` which yields the elements of this stream and discards the state.
    #[inline]
    fn into_stream(self) -> IntoStream<Self>
    where
        Self: Sized,
    {
        IntoStream(self)
    }

    /// Returns a stream which applies a transform to items of this stream.
    #[inline]
    fn map<F, B>(self, f: F) -> Map<Self, F>
    where
        Self: Sized,
        F: FnMut(Self::Item) -> B,
    {
        Map { stream: self, f: f }
    }

    /// Returns a stream which applies a transform to errors of this stream.
    #[inline]
    fn map_err<F, B>(self, f: F) -> MapErr<Self, F>
    where
        Self: Sized,
        F: FnMut(Self::Error) -> B,
    {
        MapErr { stream: self, f: f }
    }

    /// Returns a stream which applies a transform to the state of this stream.
    #[inline]
    fn map_state<F, B>(self, f: F) -> MapState<Self, F>
    where
        Self: Sized,
        F: FnOnce(Self::State) -> B,
    {
        MapState {
            stream: self,
            f: Some(f),
        }
    }

    /// Returns a stream which filters items of this stream by a predicate.
    #[inline]
    fn filter<F>(self, f: F) -> Filter<Self, F>
    where
        Self: Sized,
        F: FnMut(&Self::Item) -> bool,
    {
        Filter { stream: self, f: f }
    }

    /// Returns a stream which filters and transforms items of this stream by a predicate.
    #[inline]
    fn filter_map<F, B>(self, f: F) -> FilterMap<Self, F>
    where
        Self: Sized,
        F: FnMut(Self::Item) -> Option<B>,
    {

        FilterMap { stream: self, f: f }
    }

    /// Returns a stream which collects all items of this stream into a `Vec`, returning it along
    /// with the stream's state.
    #[inline]
    fn collect(self) -> Collect<Self>
    where
        Self: Sized,
    {
        Collect {
            stream: self,
            items: vec![],
        }
    }

    /// Returns a future which applies a closure to each item of this stream.
    #[inline]
    fn for_each<F>(self, f: F) -> ForEach<Self, F>
    where
        Self: Sized,
        F: FnMut(Self::Item),
    {
        ForEach { stream: self, f: f }
    }
}

impl<S: ?Sized> StateStream for Box<S>
where
    S: StateStream,
{
    type Item = S::Item;
    type State = S::State;
    type Error = S::Error;

    #[inline]
    fn poll(&mut self) -> Poll<StreamEvent<S::Item, S::State>, (S::Error, S::State)> {
        S::poll(self)
    }
}

impl<'a, S: ?Sized> StateStream for &'a mut S
where
    S: StateStream,
{
    type Item = S::Item;
    type State = S::State;
    type Error = S::Error;

    #[inline]
    fn poll(&mut self) -> Poll<StreamEvent<S::Item, S::State>, (S::Error, S::State)> {
        S::poll(self)
    }
}

impl<S> StateStream for AssertUnwindSafe<S>
where
    S: StateStream,
{
    type Item = S::Item;
    type State = S::State;
    type Error = S::Error;

    #[inline]
    fn poll(&mut self) -> Poll<StreamEvent<S::Item, S::State>, (S::Error, S::State)> {
        self.0.poll()
    }
}

/// An extension trait adding functionality to `Future`.
pub trait FutureExt: Future {
    /// Returns a stream which evaluates this future and then the resulting stream.
    #[inline]
    fn flatten_state_stream<E, S>(self) -> FlattenStateStream<Self>
    where
        Self: Sized + Future<Error = (E, S)>,
        Self::Item: StateStream<State = S, Error = E>,
    {
        FlattenStateStream(FlattenStateStreamState::Future(self))
    }
}

impl<F: ?Sized> FutureExt for F
where
    F: Future,
{
}

/// An extension trait adding functionality to `Stream`.
pub trait StreamExt: Stream {
    /// Returns a `StateStream` yielding the items of this stream with a state.
    #[inline]
    fn into_state_stream<S>(self, state: S) -> FromStream<Self, S>
    where
        Self: Sized,
    {
        FromStream(self, Some(state))
    }
}

impl<S: ?Sized> StreamExt for S
where
    S: Stream,
{
}

/// Returns a stream produced by iteratively applying a closure to a state.
#[inline]
pub fn unfold<T, F, Fut, It, St>(init: T, f: F) -> Unfold<T, F, Fut>
where
    F: FnMut(T) -> Fut,
    Fut: futures::IntoFuture<Item = StreamEvent<(It, T), St>>,
{
    Unfold {
        state: UnfoldState::Ready(init),
        f: f,
    }
}

/// A `StateStream` yielding elements of a normal `Stream.
pub struct FromStream<S, T>(S, Option<T>);

impl<S, T> StateStream for FromStream<S, T>
where
    S: Stream,
{
    type Item = S::Item;
    type State = T;
    type Error = S::Error;

    #[inline]
    fn poll(&mut self) -> Poll<StreamEvent<S::Item, T>, (S::Error, T)> {
        self.0
            .poll()
            .map(|a| match a {
                Async::Ready(Some(i)) => Async::Ready(StreamEvent::Next(i)),
                Async::Ready(None) => Async::Ready(StreamEvent::Done(
                    self.1.take().expect("poll called after completion"),
                )),
                Async::NotReady => Async::NotReady,
            })
            .map_err(|e| {
                (e, self.1.take().expect("poll called after completion"))
            })
    }
}

/// A future yielding the next element of a stream.
pub struct IntoFuture<S>(Option<S>);

impl<S> Future for IntoFuture<S>
where
    S: StateStream,
{
    type Item = (StreamEvent<S::Item, S::State>, S);
    type Error = (S::Error, S::State, S);

    #[inline]
    fn poll(&mut self) -> Poll<(StreamEvent<S::Item, S::State>, S), (S::Error, S::State, S)> {
        let item = match self.0.as_mut().expect("polling IntoFuture twice").poll() {
            Ok(Async::NotReady) => return Ok(Async::NotReady),
            Ok(Async::Ready(i)) => Ok(i),
            Err(e) => Err(e),
        };
        let stream = self.0.take().unwrap();
        match item {
            Ok(i) => Ok(Async::Ready((i, stream))),
            Err((e, s)) => Err((e, s, stream)),
        }
    }
}

/// A `Stream` yielding the elements of a `StateStream` and discarding its state.
pub struct IntoStream<S>(S);

impl<S> Stream for IntoStream<S>
where
    S: StateStream,
{
    type Item = S::Item;
    type Error = S::Error;

    #[inline]
    fn poll(&mut self) -> Poll<Option<S::Item>, S::Error> {
        match self.0.poll() {
            Ok(Async::Ready(StreamEvent::Next(i))) => Ok(Async::Ready(Some(i))),
            Ok(Async::Ready(StreamEvent::Done(_))) => Ok(Async::Ready(None)),
            Ok(Async::NotReady) => Ok(Async::NotReady),
            Err((e, _)) => Err(e),
        }
    }
}

/// A stream which applies a transform to the elements of a stream.
pub struct Map<S, F> {
    stream: S,
    f: F,
}

impl<S, F, B> StateStream for Map<S, F>
where
    S: StateStream,
    F: FnMut(S::Item) -> B,
{
    type Item = B;
    type State = S::State;
    type Error = S::Error;

    #[inline]
    fn poll(&mut self) -> Poll<StreamEvent<B, S::State>, (S::Error, S::State)> {
        self.stream.poll().map(|a| match a {
            Async::Ready(StreamEvent::Next(i)) => Async::Ready(StreamEvent::Next((self.f)(i))),
            Async::Ready(StreamEvent::Done(s)) => Async::Ready(StreamEvent::Done(s)),
            Async::NotReady => Async::NotReady,
        })
    }
}

/// A stream which applies a transform to the errors of a stream.
pub struct MapErr<S, F> {
    stream: S,
    f: F,
}

impl<S, F, B> StateStream for MapErr<S, F>
where
    S: StateStream,
    F: FnMut(S::Error) -> B,
{
    type Item = S::Item;
    type State = S::State;
    type Error = B;

    #[inline]
    fn poll(&mut self) -> Poll<StreamEvent<S::Item, S::State>, (B, S::State)> {
        match self.stream.poll() {
            Ok(a) => Ok(a),
            Err((e, s)) => Err(((self.f)(e), s)),
        }
    }
}

/// A stream which applies a transform to the state of a stream.
pub struct MapState<S, F> {
    stream: S,
    f: Option<F>,
}

impl<S, F, B> StateStream for MapState<S, F>
where
    S: StateStream,
    F: FnOnce(S::State) -> B,
{
    type Item = S::Item;
    type State = B;
    type Error = S::Error;

    #[inline]
    fn poll(&mut self) -> Poll<StreamEvent<S::Item, B>, (S::Error, B)> {
        self.stream
            .poll()
            .map(|a| match a {
                Async::Ready(StreamEvent::Next(i)) => Async::Ready(StreamEvent::Next(i)),
                Async::Ready(StreamEvent::Done(s)) => {
                    let f = self.f.take().expect("polled MapState after completion");
                    Async::Ready(StreamEvent::Done(f(s)))
                }
                Async::NotReady => Async::NotReady,
            })
            .map_err(|(e, s)| {
                let f = self.f.take().expect("polled MapState after completion");
                (e, f(s))
            })
    }
}

/// A future which collects the items of a stream.
pub struct Collect<S>
where
    S: StateStream,
{
    stream: S,
    items: Vec<S::Item>,
}

impl<S> Future for Collect<S>
where
    S: StateStream,
{
    type Item = (Vec<S::Item>, S::State);
    type Error = (S::Error, S::State);

    #[inline]
    fn poll(&mut self) -> Poll<(Vec<S::Item>, S::State), (S::Error, S::State)> {
        loop {
            match self.stream.poll() {
                Ok(Async::Ready(StreamEvent::Next(i))) => self.items.push(i),
                Ok(Async::Ready(StreamEvent::Done(s))) => {
                    let items = mem::replace(&mut self.items, vec![]);
                    return Ok(Async::Ready((items, s)));
                }
                Ok(Async::NotReady) => return Ok(Async::NotReady),
                Err(e) => return Err(e),
            }
        }
    }
}

/// A stream which iteratively applies a closure to state to produce items.
pub struct Unfold<T, F, Fut>
where
    Fut: futures::IntoFuture,
{
    state: UnfoldState<T, Fut::Future>,
    f: F,
}

enum UnfoldState<T, F> {
    Empty,
    Ready(T),
    Processing(F),
}

impl<T, F, Fut, It, St, E> StateStream for Unfold<T, F, Fut>
    where F: FnMut(T) -> Fut,
          Fut: futures::IntoFuture<Item = StreamEvent<(It, T), St>, Error = (E, St)>
{
    type Item = It;
    type State = St;
    type Error = E;

    #[inline]
    fn poll(&mut self) -> Poll<StreamEvent<It, St>, (E, St)> {
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
where
    F: Future,
{
    Future(F),
    Stream(F::Item),
}

/// A stream which evaluates a future and then then stream returned by it.
pub struct FlattenStateStream<F>(FlattenStateStreamState<F>)
where
    F: Future;

impl<F, E, S> StateStream for FlattenStateStream<F>
where
    F: Future<Error = (E, S)>,
    F::Item: StateStream<Error = E, State = S>,
{
    type Item = <F::Item as StateStream>::Item;
    type State = S;
    type Error = E;

    #[inline]
    fn poll(&mut self) -> Poll<StreamEvent<Self::Item, Self::State>, (Self::Error, Self::State)> {
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

/// A stream which applies a filter to the items of a stream.
pub struct Filter<S, F> {
    stream: S,
    f: F,
}

impl<S, F> StateStream for Filter<S, F>
where
    S: StateStream,
    F: FnMut(&S::Item) -> bool,
{
    type Item = S::Item;
    type State = S::State;
    type Error = S::Error;

    #[inline]
    fn poll(&mut self) -> Poll<StreamEvent<S::Item, S::State>, (S::Error, S::State)> {
        loop {
            match self.stream.poll() {
                Ok(Async::Ready(StreamEvent::Next(i))) => {
                    if (self.f)(&i) {
                        return Ok(Async::Ready(StreamEvent::Next(i)));
                    }
                }
                s => return s,
            }
        }
    }
}

/// A stream which applies a filter and transform to items of a stream.
pub struct FilterMap<S, F> {
    stream: S,
    f: F,
}

impl<S, F, B> StateStream for FilterMap<S, F>
where
    S: StateStream,
    F: FnMut(S::Item) -> Option<B>,
{
    type Item = B;
    type State = S::State;
    type Error = S::Error;

    #[inline]
    fn poll(&mut self) -> Poll<StreamEvent<B, S::State>, (S::Error, S::State)> {
        loop {
            match try!(self.stream.poll()) {
                Async::Ready(StreamEvent::Next(i)) => {
                    if let Some(i) = (self.f)(i) {
                        return Ok(Async::Ready(StreamEvent::Next(i)));
                    }
                }
                Async::Ready(StreamEvent::Done(s)) => {
                    return Ok(Async::Ready(StreamEvent::Done(s)));
                }
                Async::NotReady => return Ok(Async::NotReady),
            }
        }
    }
}

/// A future which applies a closure to each item of a stream.
pub struct ForEach<S, F> {
    stream: S,
    f: F,
}

impl<S, F> Future for ForEach<S, F>
where
    S: StateStream,
    F: FnMut(S::Item),
{
    type Item = S::State;
    type Error = (S::Error, S::State);

    #[inline]
    fn poll(&mut self) -> Poll<S::State, (S::Error, S::State)> {
        loop {
            match try_ready!(self.stream.poll()) {
                StreamEvent::Next(i) => (self.f)(i),
                StreamEvent::Done(s) => return Ok(Async::Ready(s)),
            }
        }
    }
}
