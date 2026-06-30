import React from "react";
import { Link } from "wouter";
import { useGetBlocks } from "@workspace/api-client-react";
import { formatNumber, timeAgo, formatAddress } from "@/lib/utils";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Button } from "@/components/ui/button";
import { ChevronLeft, ChevronRight } from "lucide-react";

export default function Blocks() {
  const [page, setPage] = React.useState(0);
  const limit = 20;
  
  const { data: blocks, isLoading } = useGetBlocks({ limit, offset: page * limit });

  return (
    <div className="space-y-6">
      <div className="flex justify-between items-center">
        <div>
          <h2 className="text-2xl font-bold tracking-tight">Blocks</h2>
          <p className="text-muted-foreground">Browse all blocks produced on the Zebvix network.</p>
        </div>
        <div className="flex items-center gap-2">
          <Button variant="outline" size="sm" onClick={() => setPage(p => Math.max(0, p - 1))} disabled={page === 0}>
            <ChevronLeft className="w-4 h-4 mr-1" /> Prev
          </Button>
          <span className="text-sm font-mono px-2">Page {page + 1}</span>
          <Button variant="outline" size="sm" onClick={() => setPage(p => p + 1)} disabled={!blocks || blocks.length < limit}>
            Next <ChevronRight className="w-4 h-4 ml-1" />
          </Button>
        </div>
      </div>

      <Card>
        <CardContent className="p-0">
          {isLoading ? (
            <div className="p-4 space-y-2">
              {Array.from({ length: 10 }).map((_, i) => <Skeleton key={i} className="h-12 w-full" />)}
            </div>
          ) : (
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Block</TableHead>
                  <TableHead>Age</TableHead>
                  <TableHead>Hash</TableHead>
                  <TableHead>Proposer</TableHead>
                  <TableHead>Txs</TableHead>
                  <TableHead className="text-right">Gas Used</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {blocks?.map((block) => (
                  <TableRow key={block.number}>
                    <TableCell className="font-mono">
                      <Link href={`/blocks/${block.number}`} className="text-primary hover:underline">
                        {block.number}
                      </Link>
                    </TableCell>
                    <TableCell className="text-sm text-muted-foreground">{timeAgo(block.timestamp)}</TableCell>
                    <TableCell className="font-mono text-xs text-muted-foreground">{formatAddress(block.hash, 8)}</TableCell>
                    <TableCell className="font-mono text-xs">
                      <Link href={`/validators/${block.proposer}`} className="hover:underline">
                        {formatAddress(block.proposer, 6)}
                      </Link>
                    </TableCell>
                    <TableCell>{block.txCount}</TableCell>
                    <TableCell className="text-right font-mono text-xs">
                      <div className="flex flex-col items-end">
                        <span>{formatNumber(block.gasUsed, 0)}</span>
                        <span className="text-[10px] text-muted-foreground">({((parseFloat(block.gasUsed) / parseFloat(block.gasLimit)) * 100).toFixed(1)}%)</span>
                      </div>
                    </TableCell>
                  </TableRow>
                ))}
                {blocks?.length === 0 && (
                  <TableRow>
                    <TableCell colSpan={6} className="text-center py-8 text-muted-foreground">
                      No blocks found.
                    </TableCell>
                  </TableRow>
                )}
              </TableBody>
            </Table>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
