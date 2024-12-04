# ThreadPool

A simple dependency-less scoped thread pool implemented in Rust for educational purpose.

With scoping we can ensure that the task is finished before the end of the scope, droping the
requirement for the 'static lifetime.

## Examples

```rust
use threadpool::ThreadPool;
let tp = ThreadPool::default();
let mut my_vec = vec![0, 0, 0, 0, 0, 0, 0, 0, 0];
tp.with_scope(|scope| {
    for value in &mut my_vec {
        scope.enqueue_task(|| *value += 10);
    }
});
assert_eq!(my_vec, vec![10, 10, 10, 10, 10, 10, 10, 10, 10]);
```
