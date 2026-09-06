namespace Reshot.Core.History;

/// <summary>A reversible edit (ARCHITECTURE §5).</summary>
public interface IUndoableCommand : IDisposable
{
    void Undo();
    void Redo();

    /// <summary>
    /// Releases command-owned state. Commands without disposable state can keep the default
    /// implementation; stateful commands must override it. The edited layer is never owned here.
    /// </summary>
    void IDisposable.Dispose() { }
}

/// <summary>
/// Bounded undo/redo stack (SPEC §11: 32 steps). Pushing a new command clears the
/// redo stack, as usual. Commands own their own before/after state.
/// </summary>
public sealed class UndoHistory : IDisposable
{
    private const int MaxDepth = 32;

    private readonly LinkedList<IUndoableCommand> _undo = new();
    private readonly Stack<IUndoableCommand> _redo = new();
    private readonly object _gate = new();
    private bool _disposed;

    public bool CanUndo
    {
        get
        {
            lock (_gate)
                return _undo.Count > 0;
        }
    }

    public bool CanRedo
    {
        get
        {
            lock (_gate)
                return _redo.Count > 0;
        }
    }

    public void Push(IUndoableCommand command)
    {
        lock (_gate)
        {
            ObjectDisposedException.ThrowIf(_disposed, this);
            _undo.AddLast(command);
            while (_undo.Count > MaxDepth)
            {
                var oldest = _undo.First!;
                _undo.RemoveFirst();
                DisposeCommand(oldest.Value);
            }
            ClearRedo();
        }
    }

    public bool Undo()
    {
        lock (_gate)
        {
            ObjectDisposedException.ThrowIf(_disposed, this);
            if (_undo.Last is not { } node)
                return false;

            _undo.RemoveLast();
            node.Value.Undo();
            _redo.Push(node.Value);
            return true;
        }
    }

    public bool Redo()
    {
        lock (_gate)
        {
            ObjectDisposedException.ThrowIf(_disposed, this);
            if (_redo.Count == 0)
                return false;

            var command = _redo.Pop();
            command.Redo();
            _undo.AddLast(command);
            return true;
        }
    }

    public void Dispose()
    {
        lock (_gate)
        {
            if (_disposed)
                return;
            _disposed = true;

            foreach (var command in _undo)
                DisposeCommand(command);
            foreach (var command in _redo)
                DisposeCommand(command);

            _undo.Clear();
            _redo.Clear();
        }
    }

    private void ClearRedo()
    {
        foreach (var command in _redo)
            DisposeCommand(command);
        _redo.Clear();
    }

    private static void DisposeCommand(IUndoableCommand command) => command.Dispose();
}
