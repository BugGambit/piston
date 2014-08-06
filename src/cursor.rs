
use std;
use piston::{
    GameEvent,
    KeyPress,
    KeyPressArgs,
    keyboard,
    Update,
    UpdateArgs,
};
use {
    Event,
    StartState,
    Status,
    Failure,
    Success,
    Running,
};

/// Keeps track of an event.
pub enum Cursor<'a, A, S> {
    /// Keeps track of whether a key was pressed.
    KeyPressedCursor(keyboard::Key),
    /// Keeps track of an event where you have a state of an action.
    State(&'a A, S),
    /// Keeps track of `Success` <=> `Failure`.
    InvertCursor(Box<Cursor<'a, A, S>>),
    /// Keeps track of an event where you wait and do nothing.
    WaitCursor(f64, f64),
    /// Keeps track of a `Select` event.
    SelectCursor(&'a Vec<Event<A>>, uint, Box<Cursor<'a, A, S>>),
    /// Keeps track of an event where sub events happens sequentially.
    SequenceCursor(&'a Vec<Event<A>>, uint, Box<Cursor<'a, A, S>>),
    /// Keeps track of an event where sub events are repeated sequentially.
    WhileCursor(Box<Cursor<'a, A, S>>, &'a Vec<Event<A>>, uint, Box<Cursor<'a, A, S>>),
    /// Keeps track of an event where all sub events must happen.
    WhenAllCursor(Vec<Option<Cursor<'a, A, S>>>),
}

impl<'a, A: StartState<S>, S> Cursor<'a, A, S> {
    /// Updates the cursor that tracks an event.
    ///
    /// The action need to return status and remaining delta time.
    /// Returns status and the remaining delta time.
    pub fn update(
        &mut self,
        e: &GameEvent,
        f: |dt: f64, action: &'a A, state: &mut S| -> (Status, f64)
    ) -> (Status, f64) {
        match (e, self) {
            (&KeyPress(KeyPressArgs { key: key_pressed }), &KeyPressedCursor(key)) 
            if key_pressed == key => {
                // Key press is considered to happen instantly.
                (Success, 0.0)
            },
            (&Update(UpdateArgs { dt }), &State(action, ref mut state)) => {
                // Call the function that updates the state.
                f(dt, action, state)
            },
            (_, &InvertCursor(ref mut cur)) => {
                // Invert `Success` <=> `Failure`.
                match cur.update(e, |dt, action, state| f(dt, action, state)) {
                    (Running, dt) => (Running, dt),
                    (Failure, dt) => (Success, dt),
                    (Success, dt) => (Failure, dt),
                }
            },
            (&Update(UpdateArgs { dt }), &WaitCursor(wait_t, ref mut t)) => {
                if *t + dt >= wait_t {
                    let remaining_dt = *t + dt - wait_t;
                    *t = wait_t;
                    (Success, remaining_dt)
                } else {
                    *t += dt;
                    (Running, 0.0)
                }
            },
            (_, &SelectCursor(
                seq,
                ref mut i,
                ref mut cursor
            )) => {
                let mut remaining_e = *e;
                while *i < seq.len() {
                    match cursor.update(&remaining_e, |dt, action, state| f(dt, action, state)) { 
                        (Success, x) => return (Success, x),
                        (Running, _) => { break },
                        (Failure, new_dt) => {
                            remaining_e = match *e {
                                // Change update event with remaining delta time.
                                Update(_) => Update(UpdateArgs { dt: new_dt }),
                                x => x,
                            }
                        }
                    };
                    *i += 1;
                    // If end of sequence,
                    // return the 'dt' that is left.
                    if *i >= seq.len() { return (Failure, match remaining_e {
                            Update(UpdateArgs { dt }) => dt,
                            _ => 0.0
                        }); }
                    // Create a new cursor for next event.
                    // Use the same pointer to avoid allocation.
                    **cursor = seq[*i].to_cursor();
                }
                (Running, 0.0)
            },
            (_, &SequenceCursor(
                seq, 
                ref mut i, 
                ref mut cursor
            )) => {
                let cur = cursor;
                let mut remaining_e = *e;
                while *i < seq.len() {
                    match cur.update(&remaining_e, |dt, action, state| f(dt, action, state)) {
                        (Failure, x) => return (Failure, x),
                        (Running, _) => { break },
                        (Success, new_dt) => {
                            remaining_e = match *e {
                                // Change update event with remaining delta time.
                                Update(_) => Update(UpdateArgs { dt: new_dt }),
                                // Other events are 'consumed' and not passed to next.
                                // If this is the last event, then the sequence succeeded.
                                _ => if *i == seq.len() - 1 {
                                        return (Success, new_dt) 
                                    } else {
                                        return (Running, 0.0)
                                    }
                            }
                        }
                    };
                    *i += 1;
                    // If end of sequence,
                    // return the 'dt' that is left.
                    if *i >= seq.len() { return (Success, match remaining_e {
                            Update(UpdateArgs { dt }) => dt,
                            _ => 0.0
                        }); }
                    // Create a new cursor for next event.
                    // Use the same pointer to avoid allocation.
                    **cur = seq[*i].to_cursor();
                }
                (Running, 0.0)
            },
            (_, &WhileCursor(
                ref mut ev_cursor,
                rep,
                ref mut i,
                ref mut cursor
            )) => {
                // If the event terminates, do not execute the loop.
                match ev_cursor.update(e, |dt, action, state| f(dt, action, state)) {
                    (Running, _) => {}
                    x => return x,
                };
                let cur = cursor;
                let mut remaining_e = *e;
                loop {
                    match cur.update(&remaining_e, |dt, action, state| f(dt, action, state)) {
                        (Failure, x) => return (Failure, x),
                        (Running, _) => { break },
                        (Success, new_dt) => {
                            remaining_e = match *e {
                                // Change update event with remaining delta time.
                                Update(_) => Update(UpdateArgs { dt: new_dt }),
                                // Other events are 'consumed' and not passed to next.
                                _ => return (Running, 0.0)
                            }
                        }
                    };
                    *i += 1;
                    // If end of repeated events,
                    // start over from the first one.
                    if *i >= rep.len() { *i = 0; }
                    // Create a new cursor for next event.
                    // Use the same pointer to avoid allocation.
                    **cur = rep[*i].to_cursor();
                }
                (Running, 0.0)
            },
            (_, &WhenAllCursor(ref mut cursors)) => {
                // Get the least delta time left over.
                let mut min_dt = std::f64::MAX_VALUE;
                // Count number of terminated events.
                let mut terminated = 0;
                for cur in cursors.mut_iter() {
                    match *cur {
                        None => terminated += 1,
                        Some(ref mut cur) => {
                            match cur.update(
                                e,
                                |dt, action, state| f(dt, action, state)
                            ) {
                                (Running, _) => {},
                                (Failure, new_dt) => return (Failure, new_dt),
                                (Success, new_dt) => {
                                    min_dt = min_dt.min(new_dt);
                                    terminated += 1;
                                }
                            }
                        }
                    }
                }
                match terminated {
                    // If there are no events, there is a whole 'dt' left.
                    0 if cursors.len() == 0 => (Success, match *e {
                            Update(UpdateArgs { dt }) => dt,
                            // Other kind of events happen instantly.
                            _ => 0.0
                        }),
                    // If all events terminated, the least delta time is left.
                    n if cursors.len() == n => (Success, min_dt),
                    _ => (Running, 0.0)
                }
            },
            _ => (Running, 0.0)
        }
    }
}

