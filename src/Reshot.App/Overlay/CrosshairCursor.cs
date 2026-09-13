using System.IO;
using System.Windows.Input;

namespace Reshot.App.Overlay;

/// <summary>
/// The overlay's crosshair, drawn here rather than taken from <see cref="Cursors.Cross"/>.
///
/// The stock crosshair is a single colour, so it disappears against a background of that
/// colour — a white one over a white page leaves the user aiming blind. This one is a white
/// core inside a black outline, which keeps both edges legible over anything underneath.
///
/// Built as an in-memory .cur rather than shipped as a file so there is no asset to lose,
/// and created once because a cursor costs a GDI handle.
/// </summary>
internal static class CrosshairCursor
{
    private const int Size = 32;
    private const int Hot = 16;

    private static Cursor? _cursor;

    /// <summary>The shared crosshair, falling back to the stock one if it cannot be built.</summary>
    public static Cursor Instance => _cursor ??= Build();

    private static Cursor Build()
    {
        try
        {
            using var stream = new MemoryStream();
            using var w = new BinaryWriter(stream);

            const int xorBytes = Size * Size * 4;
            const int andBytes = Size * 4;              // one bit per pixel, rows padded to 4 bytes
            const int imageBytes = 40 + xorBytes + andBytes;

            // ICONDIR
            w.Write((ushort)0);                          // reserved
            w.Write((ushort)2);                          // 2 = cursor
            w.Write((ushort)1);                          // one image

            // ICONDIRENTRY
            w.Write((byte)Size);
            w.Write((byte)Size);
            w.Write((byte)0);                            // palette size, 0 for 32bpp
            w.Write((byte)0);                            // reserved
            w.Write((ushort)Hot);                        // hotspot x
            w.Write((ushort)Hot);                        // hotspot y
            w.Write(imageBytes);
            w.Write(22);                                 // offset: 6 header + 16 entry

            // BITMAPINFOHEADER. Height is doubled because the AND mask follows the colours.
            w.Write(40);
            w.Write(Size);
            w.Write(Size * 2);
            w.Write((ushort)1);
            w.Write((ushort)32);
            w.Write(0);                                  // BI_RGB
            w.Write(0);
            w.Write(0); w.Write(0); w.Write(0); w.Write(0);

            // Colours, bottom-up as DIBs are stored.
            for (var row = Size - 1; row >= 0; row--)
            {
                for (var x = 0; x < Size; x++)
                {
                    var dx = x - Hot;
                    var dy = row - Hot;
                    var onArm = (dx is >= -1 and <= 1) || (dy is >= -1 and <= 1);
                    var onCore = dx == 0 || dy == 0;

                    if (!onArm)
                    {
                        w.Write(0);                      // transparent
                    }
                    else if (onCore)
                    {
                        w.Write(0xFFFFFFFFu);            // white core
                    }
                    else
                    {
                        w.Write(0xFF000000u);            // black outline
                    }
                }
            }

            // AND mask: unused for 32-bit cursors, alpha decides. Zero means "show the colour".
            for (var i = 0; i < andBytes; i++)
                w.Write((byte)0);

            w.Flush();
            stream.Position = 0;
            return new Cursor(stream);
        }
        catch
        {
            // A cursor that cannot be built is not worth failing a capture over.
            return Cursors.Cross;
        }
    }
}
