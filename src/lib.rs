#[macro_use]
extern crate futures;

use futures::{Async, Poll, Future, Stream, IntoFuture as IntoFutureTrait};
use std::any::Any;
use std::collections::VecDeque;
use std::mem;
use std::panic::{self, AssertUnwindSafe, UnwindSafe};

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
    fn filter<F>(self, f: F) -> Filter<Self, F>
        where Self: Sized,
              F: FnMut(&Self::Item) -> bool
    {
        Filter {
            stream: self,
            f: f,
        }
    }

    #[inline]
    fn filter_map<F, B>(self, f: F) -> FilterMap<Self, F>
        where Self: Sized,
              F: FnMut(Self::Item) -> Option<B>
    {

        FilterMap {
            stream: self,
            f: f,
        }
    }

    #[inline]
    fn then<F, U>(self, f: F) -> Then<Self, F, U>
        where Self: Sized,
              F: FnMut(Result<Self::Item, Self::Error>) -> U,
              U: futures::IntoFuture,
              Self::Error: From<U::Error>
    {
        Then {
            stream: self,
            f: f,
            fut: None,
        }
    }

    #[inline]
    fn and_then<F, U>(self, f: F) -> AndThen<Self, F, U>
        where Self: Sized,
              F: FnMut(Self::Item) -> U,
              U: futures::IntoFuture,
              Self::Error: From<U::Error>
    {
        AndThen {
            stream: self,
            f: f,
            fut: None,
        }
    }

    #[inline]
    fn and_then_state<F, U>(self, f: F) -> AndThenState<Self, F, U>
        where Self: Sized,
              F: FnMut(Self::State) -> U,
              U: futures::IntoFuture,
              Self::Error: From<U::Error>
    {
        AndThenState {
            stream: self,
            f: f,
            fut: None,
        }
    }

    #[inline]
    fn or_else<F, U>(self, f: F) -> OrElse<Self, F, U>
        where Self: Sized,
              F: FnMut(Self::Error) -> U,
              U: futures::IntoFuture<Item = Self::Item>
    {
        OrElse {
            stream: self,
            f: f,
            fut: None,
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

    #[inline]
    fn fold<T, F, Fut, G, Fut2>(self, init: T, next: F, done: G) -> Fold<Self, T, F, Fut, G, Fut2>
        where Self: Sized,
              F: FnMut(T, Self::Item) -> Fut,
              Fut: futures::IntoFuture<Item = T>,
              Self::Error: From<Fut::Error>,
              G: FnOnce(T, Self::State) -> Fut2,
              Fut2: futures::IntoFuture<Item = T>,
              Self::Error: From<Fut2::Error>
    {
        Fold {
            stream: self,
            next: next,
            state: FoldState::Ready(init, done),
        }
    }

    #[inline]
    fn skip_while<P, R>(self, pred: P) -> SkipWhile<Self, P, R>
        where Self: Sized,
              P: FnMut(&Self::Item) -> R,
              R: futures::IntoFuture<Item = bool, Error = Self::Error>
    {
        SkipWhile {
            stream: self,
            pred: pred,
            cur: None,
            done: false,
        }
    }

    #[inline]
    fn for_each<F>(self, f: F) -> ForEach<Self, F>
        where Self: Sized,
              F: FnMut(Self::Item) -> Result<(), Self::Error>
    {
        ForEach {
            stream: self,
            f: f,
        }
    }

    #[inline]
    fn catch_unwind(self) -> CatchUnwind<Self>
        where Self: Sized + UnwindSafe
    {
        CatchUnwind(self)
    }

    #[inline]
    fn buffered(self, amt: usize) -> Buffered<Self>
        where Self: Sized,
              Self::Item: futures::IntoFuture<Error = Self::Error>
    {
        assert!(amt > 0);

        Buffered {
            stream: self,
            buf: VecDeque::with_capacity(amt),
            state: BufferedStreamState::Working(amt),
        }
    }

    #[inline]
    fn buffered_unordered(self, amt: usize) -> BufferedUnordered<Self>
        where Self: Sized,
              Self::Item: futures::IntoFuture<Error = Self::Error>
    {
        assert!(amt > 0);

        let mut buf = Vec::with_capacity(amt);
        for _ in 0..amt {
            buf.push(UnorderedValueState::Empty);
        }

        BufferedUnordered {
            stream: self,
            buf: buf,
            state: UnorderedStreamState::Working,
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

impl<'a, S: ?Sized> StateStream for &'a mut S
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

impl<S> StateStream for AssertUnwindSafe<S>
    where S: StateStream
{
    type Item = S::Item;
    type State = S::State;
    type Error = S::Error;

    #[inline]
    fn poll(&mut self) -> Poll<StreamEvent<S::Item, S::State>, S::Error> {
        self.0.poll()
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

pub struct Filter<S, F> {
    stream: S,
    f: F,
}

impl<S, F> StateStream for Filter<S, F>
    where S: StateStream,
          F: FnMut(&S::Item) -> bool
{
    type Item = S::Item;
    type State = S::State;
    type Error = S::Error;

    #[inline]
    fn poll(&mut self) -> Poll<StreamEvent<S::Item, S::State>, S::Error> {
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

pub struct FilterMap<S, F> {
    stream: S,
    f: F,
}

impl<S, F, B> StateStream for FilterMap<S, F>
    where S: StateStream,
          F: FnMut(S::Item) -> Option<B>
{
    type Item = B;
    type State = S::State;
    type Error = S::Error;

    #[inline]
    fn poll(&mut self) -> Poll<StreamEvent<B, S::State>, S::Error> {
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

pub struct Then<S, F, U>
    where U: futures::IntoFuture
{
    stream: S,
    f: F,
    fut: Option<U::Future>,
}

impl<S, F, U> StateStream for Then<S, F, U>
    where S: StateStream,
          F: FnMut(Result<S::Item, S::Error>) -> U,
          U: futures::IntoFuture,
          S::Error: From<U::Error>,
{
    type Item = U::Item;
    type State = S::State;
    type Error = S::Error;

    #[inline]
    fn poll(&mut self) -> Poll<StreamEvent<U::Item, S::State>, S::Error> {
        loop {
            if let Some(mut fut) = self.fut.take() {
                match try!(fut.poll()) {
                    Async::Ready(i) => return Ok(Async::Ready(StreamEvent::Next(i))),
                    Async::NotReady => {
                        self.fut = Some(fut);
                        return Ok(Async::NotReady);
                    }
                }
            }

            match self.stream.poll() {
                Ok(Async::Ready(StreamEvent::Next(i))) => {
                    self.fut = Some((self.f)(Ok(i)).into_future());
                }
                Ok(Async::Ready(StreamEvent::Done(s))) => {
                    return Ok(Async::Ready(StreamEvent::Done(s)));
                }
                Ok(Async::NotReady) => return Ok(Async::NotReady),
                Err(e) => self.fut = Some((self.f)(Err(e)).into_future()),
            }
        }
    }
}

pub struct AndThen<S, F, U>
    where U: futures::IntoFuture
{
    stream: S,
    f: F,
    fut: Option<U::Future>,
}

impl<S, F, U> StateStream for AndThen<S, F, U>
where S: StateStream,
      F: FnMut(S::Item) -> U,
      U: futures::IntoFuture,
      S::Error: From<U::Error>,
{
    type Item = U::Item;
    type State = S::State;
    type Error = S::Error;

    #[inline]
    fn poll(&mut self) -> Poll<StreamEvent<U::Item, S::State>, S::Error> {
        loop {
            if let Some(mut fut) = self.fut.take() {
                match try!(fut.poll()) {
                    Async::Ready(i) => return Ok(Async::Ready(StreamEvent::Next(i))),
                    Async::NotReady => {
                        self.fut = Some(fut);
                        return Ok(Async::NotReady);
                    }
                }
            }

            match try!(self.stream.poll()) {
                Async::Ready(StreamEvent::Next(i)) => self.fut = Some((self.f)(i).into_future()),
                Async::Ready(StreamEvent::Done(s)) => return Ok(Async::Ready(StreamEvent::Done(s))),
                Async::NotReady => return Ok(Async::NotReady),
            }
        }
    }
}

pub struct AndThenState<S, F, U>
    where U: futures::IntoFuture
{
    stream: S,
    f: F,
    fut: Option<U::Future>,
}

impl<S, F, U> StateStream for AndThenState<S, F, U>
where S: StateStream,
      F: FnMut(S::State) -> U,
      U: futures::IntoFuture,
      S::Error: From<U::Error>,
{
    type Item = S::Item;
    type State = U::Item;
    type Error = S::Error;

    #[inline]
    fn poll(&mut self) -> Poll<StreamEvent<S::Item, U::Item>, S::Error> {
        loop {
            if let Some(mut fut) = self.fut.take() {
                match try!(fut.poll()) {
                    Async::Ready(i) => return Ok(Async::Ready(StreamEvent::Done(i))),
                    Async::NotReady => {
                        self.fut = Some(fut);
                        return Ok(Async::NotReady);
                    }
                }
            }

            match try!(self.stream.poll()) {
                Async::Ready(StreamEvent::Next(s)) => return Ok(Async::Ready(StreamEvent::Next(s))),
                Async::Ready(StreamEvent::Done(i)) => self.fut = Some((self.f)(i).into_future()),
                Async::NotReady => return Ok(Async::NotReady),
            }
        }
    }
}

pub struct OrElse<S, F, U>
    where U: futures::IntoFuture
{
    stream: S,
    f: F,
    fut: Option<U::Future>,
}

impl<S, F, U> StateStream for OrElse<S, F, U>
where S: StateStream,
      F: FnMut(S::Error) -> U,
      U: futures::IntoFuture<Item = S::Item>
{
    type Item = S::Item;
    type State = S::State;
    type Error = U::Error;

    #[inline]
    fn poll(&mut self) -> Poll<StreamEvent<S::Item, S::State>, U::Error> {
        loop {
            if let Some(mut fut) = self.fut.take() {
                match try!(fut.poll()) {
                    Async::Ready(i) => return Ok(Async::Ready(StreamEvent::Next(i))),
                    Async::NotReady => {
                        self.fut = Some(fut);
                        return Ok(Async::NotReady);
                    }
                }
            }

            match self.stream.poll() {
                Ok(Async::Ready(StreamEvent::Next(i))) => {
                    return Ok(Async::Ready(StreamEvent::Next(i)));
                }
                Ok(Async::Ready(StreamEvent::Done(s))) => {
                    return Ok(Async::Ready(StreamEvent::Done(s)));
                }
                Ok(Async::NotReady) => return Ok(Async::NotReady),
                Err(e) => self.fut = Some((self.f)(e).into_future()),
            }
        }
    }
}

enum FoldState<T, Fut, G, Fut2>
    where Fut: futures::IntoFuture,
          Fut2: futures::IntoFuture
{
    Ready(T, G),
    Processing(Fut::Future, G),
    ProcessingDone(Fut2::Future),
    Empty,
}

pub struct Fold<S, T, F, Fut, G, Fut2>
    where Fut: futures::IntoFuture,
          Fut2: futures::IntoFuture
{
    stream: S,
    next: F,
    state: FoldState<T, Fut, G, Fut2>,
}

impl<S, T, F, Fut, G, Fut2> Future for Fold<S, T, F, Fut, G, Fut2>
    where S: StateStream,
          F: FnMut(T, S::Item) -> Fut,
          Fut: futures::IntoFuture<Item = T>,
          S::Error: From<Fut::Error>,
          G: FnOnce(T, S::State) -> Fut2,
          Fut2: futures::IntoFuture<Item = T>,
          S::Error: From<Fut2::Error>
{
    type Item = T;
    type Error = S::Error;

    #[inline]
    fn poll(&mut self) -> Poll<T, S::Error> {
        loop {
            match mem::replace(&mut self.state, FoldState::Empty) {
                FoldState::Ready(t, g) => {
                    match try!(self.stream.poll()) {
                        Async::Ready(StreamEvent::Next(i)) => {
                            self.state = FoldState::Processing((self.next)(t, i).into_future(), g);
                        }
                        Async::Ready(StreamEvent::Done(s)) => {
                            self.state = FoldState::ProcessingDone(g(t, s).into_future());
                        }
                        Async::NotReady => {
                            self.state = FoldState::Ready(t, g);
                            return Ok(Async::NotReady);
                        }
                    }
                }
                FoldState::Processing(mut fut, g) => {
                    match try!(fut.poll()) {
                        Async::Ready(t) => self.state = FoldState::Ready(t, g),
                        Async::NotReady => {
                            self.state = FoldState::Processing(fut, g);
                            return Ok(Async::NotReady);
                        }
                    }
                }
                FoldState::ProcessingDone(mut fut) => {
                    match try!(fut.poll()) {
                        Async::Ready(t) => return Ok(Async::Ready(t)),
                        Async::NotReady => {
                            self.state = FoldState::ProcessingDone(fut);
                            return Ok(Async::NotReady);
                        }
                    }
                }
                FoldState::Empty => panic!("cannot poll Fold twice"),
            }
        }
    }
}

pub struct SkipWhile<S, P, R>
    where S: StateStream,
          R: futures::IntoFuture
{
    stream: S,
    pred: P,
    cur: Option<(R::Future, S::Item)>,
    done: bool,
}

impl<S, P, R> StateStream for SkipWhile<S, P, R>
    where S: StateStream,
          P: FnMut(&S::Item) -> R,
          R: futures::IntoFuture<Item = bool, Error = S::Error>
{
    type Item = S::Item;
    type State = S::State;
    type Error = S::Error;

    #[inline]
    fn poll(&mut self) -> Poll<StreamEvent<S::Item, S::State>, S::Error> {
        if self.done {
            return self.stream.poll();
        }

        loop {
            if let Some((mut fut, item)) = self.cur.take() {
                match try!(fut.poll()) {
                    Async::Ready(true) => {}
                    Async::Ready(false) => {
                        self.done = true;
                        return Ok(Async::Ready(StreamEvent::Next(item)))
                    }
                    Async::NotReady => {
                        self.cur = Some((fut, item));
                        return Ok(Async::NotReady);
                    }
                }
            }

            match try_ready!(self.stream.poll()) {
                StreamEvent::Next(i) => {
                    let fut = (self.pred)(&i).into_future();
                    self.cur = Some((fut, i));
                }
                StreamEvent::Done(s) => return Ok(Async::Ready(StreamEvent::Done(s))),
            }
        }
    }
}

pub struct ForEach<S, F> {
    stream: S,
    f: F,
}

impl<S, F> Future for ForEach<S, F>
    where S: StateStream,
          F: FnMut(S::Item) -> Result<(), S::Error>
{
    type Item = S::State;
    type Error = S::Error;

    #[inline]
    fn poll(&mut self) -> Poll<S::State, S::Error> {
        loop {
            match try_ready!(self.stream.poll()) {
                StreamEvent::Next(i) => try!((self.f)(i)),
                StreamEvent::Done(s) => return Ok(Async::Ready(s)),
            }
        }
    }
}

pub struct CatchUnwind<S>(S);

impl<S> StateStream for CatchUnwind<S>
    where S: StateStream + UnwindSafe
{
    type Item = Result<S::Item, S::Error>;
    type State = S::State;
    type Error = Box<Any + Send>;

    #[inline]
    fn poll(&mut self) -> Poll<StreamEvent<Result<S::Item, S::Error>, S::State>, Box<Any + Send>> {
        match try!(panic::catch_unwind(AssertUnwindSafe(move || self.0.poll()))) {
            Ok(Async::Ready(StreamEvent::Next(i))) => {
                Ok(Async::Ready(StreamEvent::Next(Ok(i))))
            }
            Ok(Async::Ready(StreamEvent::Done(s))) => {
                Ok(Async::Ready(StreamEvent::Done(s)))
            }
            Ok(Async::NotReady) => Ok(Async::NotReady),
            Err(e) => Ok(Async::Ready(StreamEvent::Next(Err(e)))),
        }
    }
}

enum BufferState<F>
    where F: Future
{
    Pending(F),
    Done(Result<F::Item, F::Error>),
}

enum BufferedStreamState<S> {
    Working(usize),
    Done(Option<S>),
}

pub struct Buffered<S>
    where S: StateStream,
          S::Item: futures::IntoFuture<Error=S::Error>
{
    stream: S,
    buf: VecDeque<BufferState<<S::Item as futures::IntoFuture>::Future>>,
    state: BufferedStreamState<S::State>,
}

impl<S> StateStream for Buffered<S>
    where S: StateStream,
          S::Item: futures::IntoFuture<Error=S::Error>
{
    type Item = <S::Item as futures::IntoFuture>::Item;
    type State = S::State;
    type Error = S::Error;

    #[inline]
    fn poll(&mut self) -> Poll<StreamEvent<Self::Item, Self::State>, Self::Error> {
        let amt = match self.state {
            BufferedStreamState::Working(amt) => amt,
            BufferedStreamState::Done(ref mut s) => {
                let s = s.take().expect("called poll on Buffered after completion");
                return Ok(Async::Ready(StreamEvent::Done(s)));
            }
        };

        while self.buf.len() < amt {
            match try!(self.stream.poll()) {
                Async::Ready(StreamEvent::Next(i)) => {
                    self.buf.push_back(BufferState::Pending(i.into_future()));
                }
                Async::Ready(StreamEvent::Done(s)) => {
                    self.state = BufferedStreamState::Done(Some(s));
                    break;
                }
                Async::NotReady => break,
            }
        }

        for elt in &mut self.buf {
            let value = match *elt {
                BufferState::Done(_) => continue,
                BufferState::Pending(ref mut f) => {
                    match f.poll() {
                        Ok(Async::Ready(v)) => Ok(v),
                        Ok(Async::NotReady) => continue,
                        Err(e) => Err(e),
                    }
                }
            };
            *elt = BufferState::Done(value);
        }

        match self.buf.pop_front().unwrap() {
            BufferState::Done(Ok(v)) => Ok(Async::Ready(StreamEvent::Next(v))),
            BufferState::Done(Err(e)) => Err(e),
            v @ BufferState::Pending(_) => {
                self.buf.push_front(v);
                Ok(Async::NotReady)
            }
        }
    }
}

enum UnorderedValueState<F>
    where F: Future
{
    Empty,
    Pending(F),
    Done(Result<F::Item, F::Error>),
}

enum UnorderedStreamState<S> {
    Working,
    Done(Option<S>),
}

pub struct BufferedUnordered<S>
    where S: StateStream,
          S::Item: futures::IntoFuture<Error=S::Error>
{
    stream: S,
    buf: Vec<UnorderedValueState<<S::Item as futures::IntoFuture>::Future>>,
    state: UnorderedStreamState<S::State>,
}

impl<S> StateStream for BufferedUnordered<S>
    where S: StateStream,
          S::Item: futures::IntoFuture<Error=S::Error>
{
    type Item = < S::Item as futures::IntoFuture >::Item;
    type State = S::State;
    type Error = S::Error;

    #[inline]
    fn poll(&mut self) -> Poll<StreamEvent<Self::Item, Self::State>, Self::Error> {
        if let UnorderedStreamState::Done(ref mut s) = self.state {
            let s = s.take().expect("called poll on Buffered after completion");
            return Ok(Async::Ready(StreamEvent::Done(s)));
        }

        for elt in &mut self.buf {
            if let UnorderedValueState::Empty = *elt {
                match try!(self.stream.poll()) {
                    Async::Ready(StreamEvent::Next(i)) => {
                        *elt = UnorderedValueState::Pending(i.into_future());
                    }
                    Async::Ready(StreamEvent::Done(s)) => {
                        self.state = UnorderedStreamState::Done(Some(s));
                        break;
                    }
                    Async::NotReady => break,
                }
            }
        }

        for elt in &mut self.buf {
            let value = match *elt {
                UnorderedValueState::Pending(ref mut f) => {
                    match f.poll() {
                        Ok(Async::Ready(v)) => Ok(v),
                        Ok(Async::NotReady) => continue,
                        Err(e) => Err(e),
                    }
                }
                _ => continue,
            };
            *elt = UnorderedValueState::Done(value);
        }

        for elt in &mut self.buf {
            let (ret, value) = match mem::replace(elt, UnorderedValueState::Empty) {
                UnorderedValueState::Done(Ok(v)) => {
                    (Some(Ok(Async::Ready(StreamEvent::Next(v)))), UnorderedValueState::Empty)
                }
                UnorderedValueState::Done(Err(e)) => (Some(Err(e)), UnorderedValueState::Empty),
                elt => (None, elt),
            };
            *elt = value;
            if let Some(ret) = ret {
                return ret;
            }
        }

        Ok(Async::NotReady)
    }
}
