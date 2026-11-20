#nullable enable

namespace Samples.Interop;

public interface IReadCell<out T>
{
    T Read();
}

public class Cell<T> : IReadCell<T>
{
    public Cell(T value) => Value = value;

    public static Cell<T> From(T value) => new(value);

    public T Value { get; set; }

    public T this[int index]
    {
        get => index == 0 ? Value : throw new System.IndexOutOfRangeException();
        set
        {
            if (index != 0) throw new System.IndexOutOfRangeException();
            Value = value;
        }
    }

    public T Read() => Value;

    public U SetAndReturn<U>(T value, U result)
    {
        Value = value;
        return result;
    }

    public T Exchange(T value)
    {
        T previous = Value;
        Value = value;
        return previous;
    }
}

public static class CellDefaults<T>
{
    private static Cell<T>? initial;

    public static Cell<T> Initial
    {
        get => initial ?? throw new System.InvalidOperationException();
        set => initial = value;
    }

    public static U Choose<U>(T value, U result)
    {
        Initial = Cell<T>.From(value);
        return result;
    }
}

public sealed class TextCell : Cell<string>
{
    public TextCell(string value) : base(value) { }
}

public static class Cells
{
    public static Cell<T> Create<T>(T value) => new(value);

    public static Cell<T> Copy<T>(Cell<T> cell) => new(cell.Value);

    public static Cell<string>? OptionalLabel() => null;
}
