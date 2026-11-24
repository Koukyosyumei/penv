use crate::circular_buffer::CircularBuffer;

fn filled(capacity: usize, count: i32) -> CircularBuffer<i32> {
    let mut buffer = CircularBuffer::new(capacity);
    for value in 0..count {
        buffer.enqueue(value);
    }
    buffer
}

fn assert_dequeue(buffer: &mut CircularBuffer<i32>, expected: impl IntoIterator<Item = i32>) {
    for value in expected {
        assert_eq!(buffer.try_dequeue(), Some(value));
    }
}

#[test]
fn should_enqueue_and_dequeue_without_wrap() {
    assert_dequeue(&mut filled(5, 5), 0..5);
}

#[test]
fn should_enqueue_and_dequeue_with_wrap() {
    assert_dequeue(&mut filled(5, 10), 5..10);
}

fn drained() -> CircularBuffer<i32> {
    let mut buffer = filled(5, 5);
    for _ in 0..5 {
        buffer.try_dequeue();
    }
    buffer
}

#[test]
fn should_not_dequeue_when_empty() {
    assert_eq!(drained().try_dequeue(), None);
}

#[test]
fn should_not_peek_when_empty() {
    assert_eq!(drained().try_peek(), None);
}

#[test]
fn should_not_change_count_when_peeked() {
    let buffer = filled(5, 5);
    for _ in 0..5 {
        buffer.try_peek();
    }
    assert_eq!(buffer.len(), 5);
}

#[test]
fn should_enqueue_and_peek_without_wrap() {
    let mut buffer = CircularBuffer::new(5);
    for value in 0..5 {
        buffer.enqueue(value);
        assert_eq!(buffer.try_peek(), Some(&0));
    }
}

#[test]
fn should_enqueue_and_peek_with_wrap() {
    let mut buffer = CircularBuffer::new(5);
    for value in 0_i32..10 {
        buffer.enqueue(value);
        assert_eq!(buffer.try_peek(), Some(&(value - 4).max(0)));
    }
}

#[test]
fn should_never_wrap_with_single_element() {
    let mut buffer = CircularBuffer::new(5);
    for value in 0..=10 {
        buffer.try_dequeue();
        buffer.enqueue(value);
        assert!(!buffer.is_wrapped());
    }
}

#[test]
fn should_save_without_wrap() {
    let mut buffer = CircularBuffer::new(5);
    for value in [1, 2, 3] {
        buffer.enqueue(value);
    }
    let mut values: Vec<_> = buffer.iter().copied().collect();
    values.sort_unstable();
    assert_eq!(values, [1, 2, 3]);
}

#[test]
fn should_save_without_wrap_array() {
    let mut buffer = CircularBuffer::new(5);
    let expected = [1, 2, 3, -1, 0];
    for value in &expected[..3] {
        buffer.enqueue(*value);
    }
    let mut copy = [0; 5];
    copy[3] = -1;
    buffer.copy_to(&mut copy, 0);
    assert_eq!(copy, expected);
}

#[test]
fn should_save_with_wrap() {
    assert!(filled(4, 6).iter().copied().eq([2, 3, 4, 5]));
}

#[test]
fn should_save_with_exact_size() {
    assert!(filled(4, 4).iter().copied().eq([0, 1, 2, 3]));
}

#[test]
fn should_save_with_wrap_array() {
    let mut copy = [0; 4];
    filled(4, 6).copy_to(&mut copy, 0);
    assert_eq!(copy, [2, 3, 4, 5]);
}

#[test]
fn should_detect_overflow() {
    let mut buffer = CircularBuffer::new(3);
    for value in 0..3 {
        assert!(buffer.enqueue(value));
    }
    assert!(!buffer.enqueue(3));
}

#[test]
fn should_not_detect_overflow() {
    let mut buffer = filled(3, 3);
    buffer.try_dequeue();
    assert!(buffer.enqueue(2));
}
